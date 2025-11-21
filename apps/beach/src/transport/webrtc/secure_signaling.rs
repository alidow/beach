use argon2::{Algorithm, Argon2, Params, Version};
use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Nonce};
use hkdf::Hkdf;
use rand::RngCore;
use sha2::Sha256;

use crate::transport::TransportError;

pub const SIGNALING_ENCRYPTION_VERSION: u32 = 1;
const HKDF_INFO_AEAD: &[u8] = b"beach:secure-signaling:aead:v1";
const LABEL_OFFER: &[u8] = b"offer";
const LABEL_ANSWER: &[u8] = b"answer";
const LABEL_ICE: &[u8] = b"ice";
const HANDSHAKE_HKDF_INFO: &[u8] = b"beach:secure-signaling:handshake";
const INSECURE_OVERRIDE_TOKEN: &str = "I_KNOW_THIS_IS_UNSAFE";

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SealedEnvelope {
    pub version: u32,
    pub nonce: String,
    pub ciphertext: String,
}

#[derive(Debug)]
pub enum MessageLabel {
    Offer,
    Answer,
    Ice,
}

pub fn secure_signaling_enabled() -> bool {
    !insecure_signaling_override()
}

pub fn should_encrypt(passphrase: Option<&str>) -> bool {
    secure_signaling_enabled() && passphrase.map(|p| !p.trim().is_empty()).unwrap_or(false)
}

pub fn seal_message_with_psk(
    pre_shared_key: &[u8; 32],
    handshake_id: &str,
    label: MessageLabel,
    associated: &[&str],
    plaintext: &[u8],
) -> Result<SealedEnvelope, TransportError> {
    let msg_key = derive_message_key_from_psk(pre_shared_key, handshake_id, &label)?;
    let mut nonce_bytes = [0u8; 12];
    rand::rngs::OsRng.fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from_slice(&nonce_bytes);
    let cipher = ChaCha20Poly1305::new_from_slice(&msg_key)
        .map_err(|err| TransportError::Setup(format!("invalid key: {err}")))?;
    let aad = build_aad(handshake_id, &label, associated);
    let ciphertext = cipher
        .encrypt(
            nonce,
            Payload {
                msg: plaintext,
                aad: &aad,
            },
        )
        .map_err(|err| TransportError::Setup(format!("secure signaling encrypt failed: {err}")))?;

    Ok(SealedEnvelope {
        version: SIGNALING_ENCRYPTION_VERSION,
        nonce: BASE64_STANDARD.encode(nonce_bytes),
        ciphertext: BASE64_STANDARD.encode(ciphertext),
    })
}

pub fn seal_message(
    passphrase: &str,
    handshake_id: &str,
    label: MessageLabel,
    associated: &[&str],
    plaintext: &[u8],
) -> Result<SealedEnvelope, TransportError> {
    let stretched = derive_pre_shared_key(passphrase, handshake_id)?;
    seal_message_with_psk(&stretched, handshake_id, label, associated, plaintext)
}

pub fn open_message(
    passphrase: &str,
    handshake_id: &str,
    label: MessageLabel,
    associated: &[&str],
    envelope: &SealedEnvelope,
) -> Result<Vec<u8>, TransportError> {
    let stretched = derive_pre_shared_key(passphrase, handshake_id)?;
    open_message_with_psk(&stretched, handshake_id, label, associated, envelope)
}

pub fn open_message_with_psk(
    pre_shared_key: &[u8; 32],
    handshake_id: &str,
    label: MessageLabel,
    associated: &[&str],
    envelope: &SealedEnvelope,
) -> Result<Vec<u8>, TransportError> {
    if envelope.version != SIGNALING_ENCRYPTION_VERSION {
        return Err(TransportError::Setup(format!(
            "unsupported secure signaling version {}",
            envelope.version
        )));
    }
    let msg_key = derive_message_key_from_psk(pre_shared_key, handshake_id, &label)?;
    let nonce_bytes = BASE64_STANDARD
        .decode(envelope.nonce.as_bytes())
        .map_err(|err| TransportError::Setup(format!("invalid nonce encoding: {err}")))?;
    if nonce_bytes.len() != 12 {
        return Err(TransportError::Setup("unexpected nonce length".into()));
    }
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = BASE64_STANDARD
        .decode(envelope.ciphertext.as_bytes())
        .map_err(|err| TransportError::Setup(format!("invalid ciphertext encoding: {err}")))?;
    let cipher = ChaCha20Poly1305::new_from_slice(&msg_key)
        .map_err(|err| TransportError::Setup(format!("invalid key: {err}")))?;
    let aad = build_aad(handshake_id, &label, associated);
    let plaintext = cipher
        .decrypt(
            nonce,
            Payload {
                msg: &ciphertext,
                aad: &aad,
            },
        )
        .map_err(|err| TransportError::Setup(format!("secure signaling decrypt failed: {err}")))?;
    Ok(plaintext)
}

fn derive_message_key_from_psk(
    stretched: &[u8; 32],
    handshake_id: &str,
    label: &MessageLabel,
) -> Result<[u8; 32], TransportError> {
    let salt = handshake_id.as_bytes();
    let hkdf = Hkdf::<Sha256>::new(Some(salt), stretched);
    let hkdf_label = match label {
        MessageLabel::Offer => LABEL_OFFER,
        MessageLabel::Answer => LABEL_ANSWER,
        MessageLabel::Ice => LABEL_ICE,
    };
    let mut key_bytes = [0u8; 32];
    hkdf.expand(&[HKDF_INFO_AEAD, hkdf_label].concat(), &mut key_bytes)
        .map_err(|err| {
            TransportError::Setup(format!("secure signaling hkdf expand failed: {err}"))
        })?;
    Ok(key_bytes)
}

pub fn derive_handshake_key_from_session(
    session_key: &[u8; 32],
    handshake_id: &str,
) -> Result<[u8; 32], TransportError> {
    let salt = handshake_id.as_bytes();
    let hkdf = Hkdf::<Sha256>::new(Some(salt), session_key);
    let mut key_bytes = [0u8; 32];
    hkdf.expand(HANDSHAKE_HKDF_INFO, &mut key_bytes)
        .map_err(|err| TransportError::Setup(format!("handshake hkdf expand failed: {err}")))?;
    Ok(key_bytes)
}

fn stretch_passphrase(passphrase: &str, handshake_id: &str) -> Result<[u8; 32], TransportError> {
    // Keep the KDF cost low enough to avoid handshake delays while still stretching the passphrase.
    let params = Params::new(32 * 1024, 1, 1, Some(32))
        .map_err(|err| TransportError::Setup(format!("invalid argon2 params: {err}")))?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut output = [0u8; 32];
    argon
        .hash_password_into(passphrase.as_bytes(), handshake_id.as_bytes(), &mut output)
        .map_err(|err| TransportError::Setup(format!("argon2 derive failed: {err}")))?;
    Ok(output)
}

pub fn derive_pre_shared_key(passphrase: &str, salt: &str) -> Result<[u8; 32], TransportError> {
    stretch_passphrase(passphrase, salt)
}

fn build_aad(handshake_id: &str, label: &MessageLabel, associated: &[&str]) -> Vec<u8> {
    let mut aad = Vec::with_capacity(128);
    aad.extend_from_slice(handshake_id.as_bytes());
    aad.push(0x1f);
    match label {
        MessageLabel::Offer => aad.extend_from_slice(LABEL_OFFER),
        MessageLabel::Answer => aad.extend_from_slice(LABEL_ANSWER),
        MessageLabel::Ice => aad.extend_from_slice(LABEL_ICE),
    }
    for component in associated {
        aad.push(0x1f);
        aad.extend_from_slice(component.as_bytes());
    }
    aad
}

fn insecure_signaling_override() -> bool {
    matches!(
        std::env::var("BEACH_INSECURE_SIGNALING")
            .ok()
            .map(|value| value.trim().eq(INSECURE_OVERRIDE_TOKEN)),
        Some(true)
    )
}
