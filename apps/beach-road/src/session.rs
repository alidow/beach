use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Generate a new session ID
pub fn generate_session_id() -> String {
    Uuid::new_v4().to_string()
}

/// Hash a passphrase using SHA-256
pub fn hash_passphrase(passphrase: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(passphrase.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Verify if a passphrase matches a hash
pub fn verify_passphrase(passphrase: &str, hash: &str) -> bool {
    hash_passphrase(passphrase) == hash
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test_timeout::timeout]
    fn test_session_id_generation() {
        let id1 = generate_session_id();
        let id2 = generate_session_id();
        assert_ne!(id1, id2);
        assert_eq!(id1.len(), 36); // UUID v4 format
    }

    #[test_timeout::timeout]
    fn test_passphrase_hashing() {
        let passphrase = "test_passphrase";
        let hash1 = hash_passphrase(passphrase);
        let hash2 = hash_passphrase(passphrase);

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, passphrase);
    }

    #[test_timeout::timeout]
    fn test_passphrase_verification() {
        let passphrase = "correct_pass";
        let hash = hash_passphrase(passphrase);

        assert!(verify_passphrase("correct_pass", &hash));
        assert!(!verify_passphrase("wrong_pass", &hash));
    }
}
