use crate::auth::error::AuthError;
use argon2::{Algorithm, Argon2, ParamsBuilder, Version};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chacha20poly1305::{
    ChaCha20Poly1305, Key, Nonce,
    aead::{Aead, KeyInit},
};
use rand::RngCore;
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KdfParams {
    pub memory_kib: u32,
    pub iterations: u32,
    pub parallelism: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EncryptedBlob {
    pub ciphertext: String,
    pub nonce: String,
    pub salt: String,
    pub kdf: KdfParams,
}

pub fn encrypt(passphrase: &str, plaintext: &str) -> Result<EncryptedBlob, AuthError> {
    let mut salt = [0u8; 16];
    let mut nonce_bytes = [0u8; 12];
    OsRng.fill_bytes(&mut salt);
    OsRng.fill_bytes(&mut nonce_bytes);

    let params = ParamsBuilder::new()
        .m_cost(19456)
        .t_cost(2)
        .p_cost(1)
        .build()
        .map_err(|err| AuthError::Encryption(err.to_string()))?;

    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params.clone());
    let mut key = [0u8; 32];
    argon2
        .hash_password_into(passphrase.as_bytes(), &salt, &mut key)
        .map_err(|err| AuthError::Encryption(err.to_string()))?;

    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
    let nonce = Nonce::from_slice(&nonce_bytes);
    let ciphertext = cipher
        .encrypt(nonce, plaintext.as_bytes())
        .map_err(|err| AuthError::Encryption(err.to_string()))?;

    Ok(EncryptedBlob {
        ciphertext: STANDARD.encode(ciphertext),
        nonce: STANDARD.encode(nonce_bytes),
        salt: STANDARD.encode(salt),
        kdf: KdfParams {
            memory_kib: params.m_cost(),
            iterations: params.t_cost(),
            parallelism: params.p_cost(),
        },
    })
}

pub fn decrypt(passphrase: &str, blob: &EncryptedBlob) -> Result<String, AuthError> {
    let salt = STANDARD
        .decode(&blob.salt)
        .map_err(|err| AuthError::Encryption(err.to_string()))?;
    let nonce_bytes = STANDARD
        .decode(&blob.nonce)
        .map_err(|err| AuthError::Encryption(err.to_string()))?;
    let ciphertext = STANDARD
        .decode(&blob.ciphertext)
        .map_err(|err| AuthError::Encryption(err.to_string()))?;

    if nonce_bytes.len() != 12 {
        return Err(AuthError::Encryption("invalid nonce length".into()));
    }

    let mut builder = ParamsBuilder::new();
    builder
        .m_cost(blob.kdf.memory_kib)
        .t_cost(blob.kdf.iterations)
        .p_cost(blob.kdf.parallelism);
    let params = builder
        .build()
        .map_err(|err| AuthError::Encryption(err.to_string()))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon2
        .hash_password_into(passphrase.as_bytes(), &salt, &mut key)
        .map_err(|err| AuthError::Encryption(err.to_string()))?;

    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key));
    let nonce = Nonce::from_slice(&nonce_bytes);
    let plaintext = cipher
        .decrypt(nonce, ciphertext.as_ref())
        .map_err(|err| AuthError::Encryption(err.to_string()))?;

    String::from_utf8(plaintext).map_err(|err| AuthError::Encryption(err.to_string()))
}
