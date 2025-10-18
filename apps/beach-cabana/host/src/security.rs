use std::fmt;

use argon2::Argon2;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{ChaCha20Poly1305, Key, Nonce};
use hkdf::Hkdf;
use rand::{rngs::OsRng, RngCore};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

const ENVELOPE_VERSION: u8 = 1;
const SIGNALING_LABEL: &[u8] = b"beach-cabana/signaling";
const NOISE_PSK_LABEL: &[u8] = b"beach-cabana/noise-psk";

#[derive(Debug, Error)]
pub enum SecurityError {
    #[error("passcode must not be empty")]
    EmptyPasscode,
    #[error("argon2 hashing failed: {0}")]
    Argon2(String),
    #[error("hkdf expand failure")]
    HkdfExpand,
    #[error("encryption failure")]
    Encrypt(#[source] chacha20poly1305::aead::Error),
    #[error("decryption failure")]
    Decrypt(#[source] chacha20poly1305::aead::Error),
    #[error("invalid envelope encoding")]
    InvalidEnvelope,
    #[error("invalid base64 encoding")]
    Base64(#[from] base64::DecodeError),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandshakeId([u8; 16]);

impl HandshakeId {
    pub fn random() -> Self {
        let mut bytes = [0u8; 16];
        OsRng.fill_bytes(&mut bytes);
        Self(bytes)
    }

    pub fn from_base64(value: &str) -> Result<Self, SecurityError> {
        let bytes = BASE64_STANDARD.decode(value)?;
        if bytes.len() != 16 {
            return Err(SecurityError::InvalidEnvelope);
        }
        let mut out = [0u8; 16];
        out.copy_from_slice(&bytes);
        Ok(Self(out))
    }

    pub fn to_base64(&self) -> String {
        BASE64_STANDARD.encode(self.0)
    }

    pub fn as_bytes(&self) -> &[u8; 16] {
        &self.0
    }
}

impl fmt::Display for HandshakeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.to_base64())
    }
}

#[derive(Debug, Clone)]
pub struct SessionMaterial {
    stretched_key: [u8; 32],
    passcode_digest: [u8; 32],
}

impl SessionMaterial {
    pub fn derive(session_id: &str, passcode: &str) -> Result<Self, SecurityError> {
        if passcode.is_empty() {
            return Err(SecurityError::EmptyPasscode);
        }
        let session_salt = derive_session_salt(session_id);
        let mut stretched = [0u8; 32];
        let argon = Argon2::default();
        argon
            .hash_password_into(passcode.as_bytes(), &session_salt, &mut stretched)
            .map_err(|err| SecurityError::Argon2(err.to_string()))?;

        let passcode_digest = Sha256::digest(passcode.as_bytes());
        let mut digest_bytes = [0u8; 32];
        digest_bytes.copy_from_slice(&passcode_digest);

        Ok(Self {
            stretched_key: stretched,
            passcode_digest: digest_bytes,
        })
    }

    pub fn passcode_fingerprint(&self) -> String {
        hex::encode(self.passcode_digest)
    }

    pub fn preview_signaling_key(
        &self,
        handshake_id: &HandshakeId,
    ) -> Result<[u8; 32], SecurityError> {
        self.derive_signaling_key(handshake_id)
    }

    pub fn derive_noise_psk(
        &self,
        handshake_id: &HandshakeId,
    ) -> Result<[u8; 32], SecurityError> {
        derive_key(&self.stretched_key, handshake_id.as_bytes(), NOISE_PSK_LABEL)
    }

    fn derive_signaling_key(&self, handshake_id: &HandshakeId) -> Result<[u8; 32], SecurityError> {
        derive_key(&self.stretched_key, handshake_id.as_bytes(), SIGNALING_LABEL)
    }
}

fn derive_session_salt(session_id: &str) -> [u8; 16] {
    let mut hasher = Sha256::new();
    hasher.update(b"beach-cabana-session:");
    hasher.update(session_id.as_bytes());
    let digest = hasher.finalize();
    let mut salt = [0u8; 16];
    salt.copy_from_slice(&digest[..16]);
    salt
}

fn derive_key(ikm: &[u8], salt: &[u8], info: &[u8]) -> Result<[u8; 32], SecurityError> {
    let hk = Hkdf::<Sha256>::new(Some(salt), ikm);
    let mut out = [0u8; 32];
    hk.expand(info, &mut out).map_err(|_| SecurityError::HkdfExpand)?;
    Ok(out)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SealedEnvelope {
    pub version: u8,
    pub handshake_b64: String,
    pub nonce_b64: String,
    pub ciphertext_b64: String,
}

impl SealedEnvelope {
    pub fn new(version: u8, handshake: &HandshakeId, nonce: &[u8; 12], ciphertext: Vec<u8>) -> Self {
        Self {
            version,
            handshake_b64: handshake.to_base64(),
            nonce_b64: BASE64_STANDARD.encode(nonce),
            ciphertext_b64: BASE64_STANDARD.encode(ciphertext),
        }
    }

    pub fn compact_encoding(&self) -> String {
        format!(
            "{}:{}:{}:{}",
            self.version, self.handshake_b64, self.nonce_b64, self.ciphertext_b64
        )
    }

    pub fn from_compact(encoded: &str) -> Result<Self, SecurityError> {
        let mut parts = encoded.split(':');
        let version = parts
            .next()
            .and_then(|v| v.parse::<u8>().ok())
            .ok_or(SecurityError::InvalidEnvelope)?;
        let handshake_b64 = parts
            .next()
            .ok_or(SecurityError::InvalidEnvelope)?
            .to_string();
        let nonce_b64 = parts
            .next()
            .ok_or(SecurityError::InvalidEnvelope)?
            .to_string();
        let ciphertext_b64 = parts
            .next()
            .ok_or(SecurityError::InvalidEnvelope)?
            .to_string();
        if parts.next().is_some() {
            return Err(SecurityError::InvalidEnvelope);
        }
        Ok(Self {
            version,
            handshake_b64,
            nonce_b64,
            ciphertext_b64,
        })
    }

    fn handshake(&self) -> Result<HandshakeId, SecurityError> {
        HandshakeId::from_base64(&self.handshake_b64)
    }

    fn nonce(&self) -> Result<[u8; 12], SecurityError> {
        let bytes = BASE64_STANDARD.decode(&self.nonce_b64)?;
        if bytes.len() != 12 {
            return Err(SecurityError::InvalidEnvelope);
        }
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&bytes);
        Ok(nonce)
    }

    fn ciphertext(&self) -> Result<Vec<u8>, SecurityError> {
        Ok(BASE64_STANDARD.decode(&self.ciphertext_b64)?)
    }
}

pub fn seal_signaling_payload(
    material: &SessionMaterial,
    handshake_id: &HandshakeId,
    plaintext: &[u8],
) -> Result<SealedEnvelope, SecurityError> {
    let key_bytes = material.derive_signaling_key(handshake_id)?;
    let key = Key::from_slice(&key_bytes);
    let cipher = ChaCha20Poly1305::new(key);
    let mut nonce = [0u8; 12];
    OsRng.fill_bytes(&mut nonce);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), Payload { msg: plaintext, aad: &[] })
        .map_err(SecurityError::Encrypt)?;
    Ok(SealedEnvelope::new(
        ENVELOPE_VERSION,
        handshake_id,
        &nonce,
        ciphertext,
    ))
}

pub fn open_signaling_payload(
    material: &SessionMaterial,
    envelope: &SealedEnvelope,
) -> Result<Vec<u8>, SecurityError> {
    if envelope.version != ENVELOPE_VERSION {
        return Err(SecurityError::InvalidEnvelope);
    }
    let handshake = envelope.handshake()?;
    let key_bytes = material.derive_signaling_key(&handshake)?;
    let key = Key::from_slice(&key_bytes);
    let cipher = ChaCha20Poly1305::new(key);
    let nonce = envelope.nonce()?;
    let ciphertext = envelope.ciphertext()?;
    cipher
        .decrypt(Nonce::from_slice(&nonce), Payload { msg: &ciphertext, aad: &[] })
        .map_err(SecurityError::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derive_noise_psk_is_deterministic_per_inputs() {
        let material = SessionMaterial::derive("session-abc", "passcode123").expect("material");
        let handshake_a =
            HandshakeId::from_base64("AAAAAAAAAAAAAAAAAAAAAA==").expect("handshake parse");
        let psk_a = material.derive_noise_psk(&handshake_a).expect("psk");
        let psk_b = material.derive_noise_psk(&handshake_a).expect("psk");
        assert_eq!(psk_a, psk_b);

        let handshake_b =
            HandshakeId::from_base64("/////////////////////w==").expect("handshake parse");
        let psk_c = material.derive_noise_psk(&handshake_b).expect("psk");
        assert_ne!(psk_a, psk_c);
    }
}

