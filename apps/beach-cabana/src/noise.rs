//! Noise XXpsk2 handshake primitives for Cabana's zero-trust media pipeline.
//!
//! This module keeps the secure transport building blocks in the Cabana crate so
//! the eventual WebRTC integration can focus on wiring messages over the data
//! channel rather than re-deriving cryptographic details from scratch.

use std::collections::VecDeque;
use std::fmt;

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use sha2::Sha256;
use snow::error::Error as SnowError;
use snow::params::NoiseParams;
use snow::Builder as NoiseBuilder;
use thiserror::Error;

use crate::security::{HandshakeId, SecurityError, SessionMaterial};

const NOISE_PROTOCOL: &str = "Noise_XXpsk2_25519_ChaChaPoly_BLAKE2s";
const PROLOGUE_PREFIX: &[u8] = b"beach-cabana/noise/v1";
const MEDIA_DIRECTION_PREFIX: &str = "beach-cabana/media-direction:";
const MEDIA_VERIFY_PREFIX: &str = "beach-cabana/media-verify:";
const MEDIA_FRAME_VERSION: u8 = 1;

/// Role of the peer in the Noise handshake.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandshakeRole {
    Initiator,
    Responder,
}

/// Configuration for a Noise handshake.
pub struct HandshakeConfig<'a> {
    pub material: &'a SessionMaterial,
    pub handshake_id: &'a HandshakeId,
    pub role: HandshakeRole,
    pub local_id: &'a str,
    pub remote_id: &'a str,
    pub prologue_context: &'a [u8],
}

impl fmt::Debug for HandshakeConfig<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HandshakeConfig")
            .field("role", &self.role)
            .field("local_id", &self.local_id)
            .field("remote_id", &self.remote_id)
            .field("handshake_id", &self.handshake_id.to_base64())
            .finish_non_exhaustive()
    }
}

#[derive(Debug, Error)]
pub enum NoiseError {
    #[error("security error: {0}")]
    Security(#[from] SecurityError),
    #[error("invalid noise parameters: {0}")]
    InvalidParams(String),
    #[error("noise handshake failure: {0}")]
    Handshake(String),
    #[error("handshake not finished")]
    Incomplete,
    #[error("media cipher failure: {0}")]
    Cipher(String),
    #[error("received stale or replayed media frame (counter: {0})")]
    Replay(u64),
    #[error("unsupported media frame version: {0}")]
    UnsupportedFrame(u8),
    #[error("invalid media frame: {0}")]
    InvalidFrame(String),
}

/// Wraps a Noise handshake state along with metadata required to derive transport keys.
pub struct NoiseHandshake {
    state: snow::HandshakeState,
    local_id: String,
    remote_id: String,
    psk: [u8; 32],
}

impl NoiseHandshake {
    pub fn new(config: HandshakeConfig<'_>) -> Result<Self, NoiseError> {
        let params: NoiseParams = NOISE_PROTOCOL
            .parse()
            .map_err(|err: SnowError| NoiseError::InvalidParams(err.to_string()))?;
        let psk = config.material.derive_noise_psk(config.handshake_id)?;
        let prologue =
            build_prologue(config.handshake_id, config.local_id, config.remote_id, config.prologue_context);

        let mut builder = NoiseBuilder::new(params).prologue(&prologue);
        builder = builder.psk(2, &psk);

        let keypair = builder
            .generate_keypair()
            .map_err(|err| NoiseError::Handshake(err.to_string()))?;
        builder = builder.local_private_key(&keypair.private);

        let state = match config.role {
            HandshakeRole::Initiator => builder
                .build_initiator()
                .map_err(|err| NoiseError::Handshake(err.to_string()))?,
            HandshakeRole::Responder => builder
                .build_responder()
                .map_err(|err| NoiseError::Handshake(err.to_string()))?,
        };

        Ok(Self {
            state,
            local_id: config.local_id.to_string(),
            remote_id: config.remote_id.to_string(),
            psk,
        })
    }

    pub fn is_finished(&self) -> bool {
        self.state.is_handshake_finished()
    }

    pub fn write_message(&mut self, payload: &[u8]) -> Result<Vec<u8>, NoiseError> {
        let mut buf = vec![0u8; 1024];
        let len = self
            .state
            .write_message(payload, &mut buf)
            .map_err(|err| NoiseError::Handshake(err.to_string()))?;
        buf.truncate(len);
        Ok(buf)
    }

    pub fn read_message(&mut self, message: &[u8]) -> Result<Vec<u8>, NoiseError> {
        let mut buf = vec![0u8; 1024];
        let len = self
            .state
            .read_message(message, &mut buf)
            .map_err(|err| NoiseError::Handshake(err.to_string()))?;
        buf.truncate(len);
        Ok(buf)
    }

    pub fn finalize(self) -> Result<NoiseSession, NoiseError> {
        let state = self.state;
        if !state.is_handshake_finished() {
            return Err(NoiseError::Incomplete);
        }
        let handshake_hash = state.get_handshake_hash().to_vec();
        state
            .into_transport_mode()
            .map_err(|err| NoiseError::Handshake(err.to_string()))?;

        let keys = derive_transport_keys(
            &self.psk,
            &handshake_hash,
            &self.local_id,
            &self.remote_id,
        )?;

        Ok(NoiseSession {
            handshake_hash,
            keys,
        })
    }
}

/// Transport keys derived from the Noise handshake.
#[derive(Clone, Debug)]
pub struct TransportKeys {
    pub send_key: [u8; 32],
    pub recv_key: [u8; 32],
    pub verification_code: String,
}

impl TransportKeys {
    pub fn encryptor(&self, handshake_hash: &[u8]) -> MediaEncryptor {
        MediaEncryptor::new(&self.send_key, handshake_hash)
    }

    pub fn decryptor(&self, handshake_hash: &[u8]) -> MediaDecryptor {
        MediaDecryptor::new(&self.recv_key, handshake_hash)
    }
}

/// Result of a completed Noise handshake.
#[derive(Clone, Debug)]
pub struct NoiseSession {
    pub handshake_hash: Vec<u8>,
    pub keys: TransportKeys,
}

impl NoiseSession {
    pub fn verification_code(&self) -> &str {
        &self.keys.verification_code
    }
}

pub struct MediaFrame {
    pub version: u8,
    pub nonce: [u8; 12],
    pub ciphertext: Vec<u8>,
}

impl MediaFrame {
    pub fn encode(&self) -> Vec<u8> {
        let mut buf = Vec::with_capacity(1 + 12 + 4 + self.ciphertext.len());
        buf.push(self.version);
        buf.extend_from_slice(&self.nonce);
        buf.extend_from_slice(&(self.ciphertext.len() as u32).to_be_bytes());
        buf.extend_from_slice(&self.ciphertext);
        buf
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, NoiseError> {
        if bytes.len() < 17 {
            return Err(NoiseError::InvalidFrame("frame too short".into()));
        }
        let version = bytes[0];
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&bytes[1..13]);
        let mut len_bytes = [0u8; 4];
        len_bytes.copy_from_slice(&bytes[13..17]);
        let ciphertext_len = u32::from_be_bytes(len_bytes) as usize;
        if bytes.len() != 17 + ciphertext_len {
            return Err(NoiseError::InvalidFrame("ciphertext length mismatch".into()));
        }
        let ciphertext = bytes[17..].to_vec();
        Ok(Self {
            version,
            nonce,
            ciphertext,
        })
    }
}

pub struct MediaEncryptor {
    cipher: ChaCha20Poly1305,
    counter: u64,
    aad: Vec<u8>,
}

impl MediaEncryptor {
    fn new(key: &[u8; 32], handshake_hash: &[u8]) -> Self {
        Self {
            cipher: ChaCha20Poly1305::new(Key::from_slice(key)),
            counter: 0,
            aad: handshake_hash.to_vec(),
        }
    }

    pub fn seal(&mut self, plaintext: &[u8]) -> Result<MediaFrame, NoiseError> {
        let nonce = next_nonce(&mut self.counter);
        let ciphertext = self
            .cipher
            .encrypt(Nonce::from_slice(&nonce), Payload { msg: plaintext, aad: &self.aad })
            .map_err(|err| NoiseError::Cipher(err.to_string()))?;
        Ok(MediaFrame {
            version: MEDIA_FRAME_VERSION,
            nonce,
            ciphertext,
        })
    }
}

pub struct MediaDecryptor {
    cipher: ChaCha20Poly1305,
    last_counter: Option<u64>,
    aad: Vec<u8>,
}

impl MediaDecryptor {
    fn new(key: &[u8; 32], handshake_hash: &[u8]) -> Self {
        Self {
            cipher: ChaCha20Poly1305::new(Key::from_slice(key)),
            last_counter: None,
            aad: handshake_hash.to_vec(),
        }
    }

    pub fn open(&mut self, frame: &MediaFrame) -> Result<Vec<u8>, NoiseError> {
        if frame.version != MEDIA_FRAME_VERSION {
            return Err(NoiseError::UnsupportedFrame(frame.version));
        }

        let counter = counter_from_nonce(&frame.nonce);
        if let Some(last) = self.last_counter {
            if counter <= last {
                return Err(NoiseError::Replay(counter));
            }
        }

        let plaintext = self
            .cipher
            .decrypt(Nonce::from_slice(&frame.nonce), Payload { msg: &frame.ciphertext, aad: &self.aad })
            .map_err(|err| NoiseError::Cipher(err.to_string()))?;
        self.last_counter = Some(counter);
        Ok(plaintext)
    }
}

fn derive_transport_keys(
    psk: &[u8; 32],
    handshake_hash: &[u8],
    local_id: &str,
    remote_id: &str,
) -> Result<TransportKeys, NoiseError> {
    let hkdf = Hkdf::<Sha256>::new(Some(psk), handshake_hash);

    let send_label = format!("{MEDIA_DIRECTION_PREFIX}{local_id}->{remote_id}");
    let recv_label = format!("{MEDIA_DIRECTION_PREFIX}{remote_id}->{local_id}");

    let mut send_key = [0u8; 32];
    let mut recv_key = [0u8; 32];
    hkdf.expand(send_label.as_bytes(), &mut send_key)
        .map_err(|_| NoiseError::Cipher("hkdf expand failure".into()))?;
    hkdf.expand(recv_label.as_bytes(), &mut recv_key)
        .map_err(|_| NoiseError::Cipher("hkdf expand failure".into()))?;

    let mut peers = [local_id.to_string(), remote_id.to_string()];
    peers.sort();
    let verify_label = format!("{MEDIA_VERIFY_PREFIX}{}|{}", peers[0], peers[1]);
    let mut verify_bytes = [0u8; 4];
    hkdf.expand(verify_label.as_bytes(), &mut verify_bytes)
        .map_err(|_| NoiseError::Cipher("hkdf expand failure".into()))?;
    let code = u32::from_le_bytes(verify_bytes) % 1_000_000;
    let verification_code = format!("{code:06}");

    Ok(TransportKeys {
        send_key,
        recv_key,
        verification_code,
    })
}

fn build_prologue(
    handshake_id: &HandshakeId,
    local_id: &str,
    remote_id: &str,
    context: &[u8],
) -> Vec<u8> {
    let mut peers = [local_id.as_bytes(), remote_id.as_bytes()];
    peers.sort();

    let mut prologue = Vec::with_capacity(
        PROLOGUE_PREFIX.len() + handshake_id.to_base64().len() + peers[0].len() + peers[1].len() + context.len() + 5,
    );
    prologue.extend_from_slice(PROLOGUE_PREFIX);
    prologue.push(0x1f);
    prologue.extend_from_slice(handshake_id.to_base64().as_bytes());
    prologue.push(0x1f);
    prologue.extend_from_slice(peers[0]);
    prologue.push(0x1f);
    prologue.extend_from_slice(peers[1]);
    prologue.push(0x1f);
    prologue.extend_from_slice(context);
    prologue
}

fn next_nonce(counter: &mut u64) -> [u8; 12] {
    let value = *counter;
    *counter = counter.wrapping_add(1);
    counter_to_nonce(value)
}

fn counter_from_nonce(nonce: &[u8; 12]) -> u64 {
    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(&nonce[4..]);
    u64::from_be_bytes(bytes)
}

fn counter_to_nonce(counter: u64) -> [u8; 12] {
    let mut nonce = [0u8; 12];
    nonce[4..].copy_from_slice(&counter.to_be_bytes());
    nonce
}

pub struct NoiseController {
    handshake: Option<NoiseHandshake>,
    pending_outgoing: VecDeque<Vec<u8>>,
    encryptor: Option<MediaEncryptor>,
    decryptor: Option<MediaDecryptor>,
    transport_keys: Option<TransportKeys>,
    handshake_hash: Option<Vec<u8>>,
    verification_code: Option<String>,
}

impl NoiseController {
    pub fn new(config: HandshakeConfig<'_>) -> Result<Self, NoiseError> {
        let role = config.role;
        let mut handshake = NoiseHandshake::new(config)?;
        let mut pending_outgoing = VecDeque::new();
        if matches!(role, HandshakeRole::Initiator) {
            let message = handshake.write_message(&[])?;
            pending_outgoing.push_back(message);
        }
        Ok(Self {
            handshake: Some(handshake),
            pending_outgoing,
            encryptor: None,
            decryptor: None,
            transport_keys: None,
            handshake_hash: None,
            verification_code: None,
        })
    }

    fn finish_handshake(&mut self, handshake: NoiseHandshake) -> Result<(), NoiseError> {
        let session = handshake.finalize()?;
        let NoiseSession {
            handshake_hash,
            keys,
        } = session;
        let encryptor = keys.encryptor(&handshake_hash);
        let decryptor = keys.decryptor(&handshake_hash);
        self.verification_code = Some(keys.verification_code.clone());
        self.handshake_hash = Some(handshake_hash.clone());
        self.encryptor = Some(encryptor);
        self.decryptor = Some(decryptor);
        self.transport_keys = Some(keys);
        Ok(())
    }

    pub fn take_outgoing(&mut self) -> Option<Vec<u8>> {
        self.pending_outgoing.pop_front()
    }

    pub fn process_incoming(&mut self, message: &[u8]) -> Result<Option<Vec<u8>>, NoiseError> {
        if self.handshake.is_some() {
            let finalize = {
                let handshake = self.handshake.as_mut().expect("handshake present");
                handshake.read_message(message)?;
                let mut finished = handshake.is_finished();
                if !finished {
                    let outbound = handshake.write_message(&[])?;
                    finished = handshake.is_finished();
                    self.pending_outgoing.push_back(outbound);
                }
                finished
            };
            if finalize {
                let handshake = self.handshake.take().expect("handshake present");
                self.finish_handshake(handshake)?;
            }
            Ok(None)
        } else {
            let frame = MediaFrame::decode(message)?;
            let decryptor = self
                .decryptor
                .as_mut()
                .ok_or_else(|| NoiseError::Incomplete)?;
            let plaintext = decryptor.open(&frame)?;
            Ok(Some(plaintext))
        }
    }

    pub fn handshake_complete(&self) -> bool {
        self.encryptor.is_some() && self.decryptor.is_some()
    }

    pub fn verification_code(&self) -> Option<&str> {
        self.verification_code.as_deref()
    }

    pub fn handshake_hash(&self) -> Option<&[u8]> {
        self.handshake_hash.as_deref()
    }

    pub fn transport_keys(&self) -> Option<&TransportKeys> {
        self.transport_keys.as_ref()
    }

    pub fn seal_media(&mut self, plaintext: &[u8]) -> Result<Vec<u8>, NoiseError> {
        let encryptor = self
            .encryptor
            .as_mut()
            .ok_or_else(|| NoiseError::Incomplete)?;
        let frame = encryptor.seal(plaintext)?;
        Ok(frame.encode())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn material() -> SessionMaterial {
        SessionMaterial::derive("session-demo", "correct horse battery staple").expect("material")
    }

    fn handshake_id() -> HandshakeId {
        HandshakeId::from_base64("Q29udGV4dEhhbmRzaGFrZQ==").expect("handshake")
    }

    #[test]
    fn handshake_round_trip_and_media_encryption() {
        let material = material();
        let handshake_id = handshake_id();

        let mut initiator = NoiseHandshake::new(HandshakeConfig {
            material: &material,
            handshake_id: &handshake_id,
            role: HandshakeRole::Initiator,
            local_id: "host",
            remote_id: "viewer",
            prologue_context: b"cabana-test",
        })
        .expect("initiator");

        let mut responder = NoiseHandshake::new(HandshakeConfig {
            material: &material,
            handshake_id: &handshake_id,
            role: HandshakeRole::Responder,
            local_id: "viewer",
            remote_id: "host",
            prologue_context: b"cabana-test",
        })
        .expect("responder");

        // Message 1 (initiator -> responder)
        let msg1 = initiator.write_message(&[]).expect("write1");
        responder.read_message(&msg1).expect("read1");

        // Message 2 (responder -> initiator)
        let msg2 = responder.write_message(&[]).expect("write2");
        initiator.read_message(&msg2).expect("read2");

        // Message 3 (initiator -> responder)
        let msg3 = initiator.write_message(&[]).expect("write3");
        responder.read_message(&msg3).expect("read3");

        assert!(initiator.is_finished());
        assert!(responder.is_finished());

        let session_initiator = initiator.finalize().expect("final initiator");
        let session_responder = responder.finalize().expect("final responder");

        assert_eq!(
            session_initiator.keys.send_key,
            session_responder.keys.recv_key
        );
        assert_eq!(
            session_initiator.keys.recv_key,
            session_responder.keys.send_key
        );
        assert_eq!(
            session_initiator.verification_code(),
            session_responder.verification_code()
        );

        let mut encryptor =
            session_initiator.keys.encryptor(&session_initiator.handshake_hash);
        let mut decryptor =
            session_responder.keys.decryptor(&session_responder.handshake_hash);

        let frame1 = encryptor.seal(b"frame-one").expect("seal1");
        let frame2 = encryptor.seal(b"frame-two").expect("seal2");

        let plain1 = decryptor.open(&frame1).expect("open1");
        assert_eq!(plain1, b"frame-one");
        let plain2 = decryptor.open(&frame2).expect("open2");
        assert_eq!(plain2, b"frame-two");

        // Replaying the first frame should now fail due to the counter monotonic check.
        let err = decryptor.open(&frame1).expect_err("replay");
        assert!(matches!(err, NoiseError::Replay(_)));
    }

    #[test]
    fn controller_negotiates_and_transports_media() {
        let material = material();
        let handshake_id = handshake_id();
        let context = b"controller-demo".to_vec();

        let mut host = NoiseController::new(HandshakeConfig {
            material: &material,
            handshake_id: &handshake_id,
            role: HandshakeRole::Initiator,
            local_id: "host",
            remote_id: "viewer",
            prologue_context: &context,
        })
        .expect("host controller");
        let mut viewer = NoiseController::new(HandshakeConfig {
            material: &material,
            handshake_id: &handshake_id,
            role: HandshakeRole::Responder,
            local_id: "viewer",
            remote_id: "host",
            prologue_context: &context,
        })
        .expect("viewer controller");

        let msg1 = host.take_outgoing().expect("message 1");
        assert!(viewer.process_incoming(&msg1).expect("process1").is_none());
        let msg2 = viewer.take_outgoing().expect("message 2");
        assert!(host.process_incoming(&msg2).expect("process2").is_none());
        let msg3 = host.take_outgoing().expect("message 3");
        assert!(viewer.process_incoming(&msg3).expect("process3").is_none());

        assert!(host.handshake_complete());
        assert!(viewer.handshake_complete());
        assert_eq!(host.verification_code(), viewer.verification_code());
        assert_eq!(
            host.handshake_hash()
                .map(hex::encode),
            viewer.handshake_hash().map(hex::encode)
        );

        let payload = b"encrypted-frame";
        let outbound = host.seal_media(payload).expect("seal");
        let decoded = viewer
            .process_incoming(&outbound)
            .expect("decrypt")
            .expect("plaintext");
        assert_eq!(decoded, payload);

        let reply = viewer.seal_media(b"reply-frame").expect("seal reply");
        let reply_plain = host
            .process_incoming(&reply)
            .expect("decrypt reply")
            .expect("plaintext");
        assert_eq!(reply_plain, b"reply-frame");
    }
}
