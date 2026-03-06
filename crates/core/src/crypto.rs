use aes_gcm::{
    aead::{Aead, OsRng},
    AeadCore, Aes256Gcm, KeyInit,
};
use base64::Engine;
use hkdf::Hkdf;
use sha2::{Digest, Sha256};

use crate::error::SyncError;

const HKDF_SALT: &[u8] = b"nisaba-config-v1";
const HKDF_INFO: &[u8] = b"aes-key";
const ENC_PREFIX: &str = "ENC:";

/// Derive a 256-bit AES key from a company secret using HKDF-SHA256.
pub fn derive_key(secret: &str) -> [u8; 32] {
    let hk = Hkdf::<Sha256>::new(Some(HKDF_SALT), secret.as_bytes());
    let mut key = [0u8; 32];
    hk.expand(HKDF_INFO, &mut key)
        .expect("HKDF expand should never fail for 32 bytes");
    key
}

/// Encrypt plaintext with AES-256-GCM. Returns "ENC:base64(nonce + ciphertext + tag)".
pub fn encrypt(plaintext: &str, key: &[u8; 32]) -> String {
    let cipher = Aes256Gcm::new(key.into());
    let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
    let ciphertext = cipher
        .encrypt(&nonce, plaintext.as_bytes())
        .expect("AES-256-GCM encryption should not fail");

    let mut blob = Vec::with_capacity(nonce.len() + ciphertext.len());
    blob.extend_from_slice(&nonce);
    blob.extend_from_slice(&ciphertext);

    let encoded = base64::engine::general_purpose::STANDARD.encode(&blob);
    format!("{ENC_PREFIX}{encoded}")
}

/// Decrypt a value produced by `encrypt()`. If the value doesn't start with "ENC:", returns it as-is.
pub fn decrypt(ciphertext: &str, key: &[u8; 32]) -> Result<String, SyncError> {
    let Some(encoded) = ciphertext.strip_prefix(ENC_PREFIX) else {
        // Not encrypted — return as-is (backwards compat)
        return Ok(ciphertext.to_string());
    };

    let blob = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| SyncError::ConfigError(format!("Base64 decode failed: {e}")))?;

    if blob.len() < 12 {
        return Err(SyncError::ConfigError(
            "Encrypted blob too short (missing nonce)".into(),
        ));
    }

    let (nonce_bytes, ciphertext_bytes) = blob.split_at(12);
    let nonce = aes_gcm::Nonce::from_slice(nonce_bytes);
    let cipher = Aes256Gcm::new(key.into());

    let plaintext = cipher
        .decrypt(nonce, ciphertext_bytes)
        .map_err(|_| SyncError::ConfigError("Decryption failed (wrong key?)".into()))?;

    String::from_utf8(plaintext)
        .map_err(|e| SyncError::ConfigError(format!("Decrypted data is not valid UTF-8: {e}")))
}

/// SHA-256 hex hash of a secret (for verification without storing the secret itself).
pub fn hash_secret(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    let result = hasher.finalize();
    hex::encode(&result)
}

/// Store a company secret in the OS keyring.
pub fn store_secret_in_keyring(company_id: &str, secret: &str) -> Result<(), SyncError> {
    let entry = keyring::Entry::new("nisaba", company_id)
        .map_err(|e| SyncError::ConfigError(format!("Keyring entry error: {e}")))?;
    entry
        .set_password(secret)
        .map_err(|e| SyncError::ConfigError(format!("Failed to store secret in keyring: {e}")))
}

/// Load a company secret from the OS keyring.
pub fn load_secret_from_keyring(company_id: &str) -> Result<String, SyncError> {
    let entry = keyring::Entry::new("nisaba", company_id)
        .map_err(|e| SyncError::ConfigError(format!("Keyring entry error: {e}")))?;
    entry
        .get_password()
        .map_err(|e| SyncError::ConfigError(format!("Failed to load secret from keyring: {e}")))
}

/// Delete a company secret from the OS keyring.
pub fn delete_secret_from_keyring(company_id: &str) -> Result<(), SyncError> {
    let entry = keyring::Entry::new("nisaba", company_id)
        .map_err(|e| SyncError::ConfigError(format!("Keyring entry error: {e}")))?;
    match entry.delete_credential() {
        Ok(()) => Ok(()),
        Err(keyring::Error::NoEntry) => Ok(()), // already gone
        Err(e) => Err(SyncError::ConfigError(format!(
            "Failed to delete secret from keyring: {e}"
        ))),
    }
}

/// Helper for hex encoding (avoids adding the `hex` crate).
mod hex {
    pub fn encode(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{b:02x}")).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let secret = "test-secret-uuid-here";
        let key = derive_key(secret);
        let plaintext = r#"{"ebay":{"client_id":"abc123"}}"#;

        let encrypted = encrypt(plaintext, &key);
        assert!(encrypted.starts_with("ENC:"));
        assert_ne!(encrypted, plaintext);

        let decrypted = decrypt(&encrypted, &key).unwrap();
        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_decrypt_unencrypted_passthrough() {
        let key = derive_key("anything");
        let plain = "just a plain string";
        let result = decrypt(plain, &key).unwrap();
        assert_eq!(result, plain);
    }

    #[test]
    fn test_decrypt_wrong_key_fails() {
        let key1 = derive_key("secret-1");
        let key2 = derive_key("secret-2");
        let encrypted = encrypt("hello", &key1);
        assert!(decrypt(&encrypted, &key2).is_err());
    }

    #[test]
    fn test_hash_secret_deterministic() {
        let h1 = hash_secret("my-company-secret");
        let h2 = hash_secret("my-company-secret");
        assert_eq!(h1, h2);
        assert_eq!(h1.len(), 64); // SHA-256 hex = 64 chars
    }

    #[test]
    fn test_hash_secret_different_inputs() {
        let h1 = hash_secret("secret-a");
        let h2 = hash_secret("secret-b");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_derive_key_deterministic() {
        let k1 = derive_key("test");
        let k2 = derive_key("test");
        assert_eq!(k1, k2);
    }

    #[test]
    fn test_each_encryption_unique() {
        let key = derive_key("test");
        let e1 = encrypt("same", &key);
        let e2 = encrypt("same", &key);
        // Different nonces → different ciphertexts
        assert_ne!(e1, e2);
        // But both decrypt to the same value
        assert_eq!(decrypt(&e1, &key).unwrap(), "same");
        assert_eq!(decrypt(&e2, &key).unwrap(), "same");
    }
}
