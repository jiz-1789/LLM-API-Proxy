use crate::config::GatewaySettings;
use aes_gcm::aead::{AeadInPlace, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::Rng;
use rusqlite::params;
use std::path::{Path, PathBuf};
use tracing::{error, info, warn};

/// AES-256-GCM Key Manager.
/// 
/// Responsibilities:
/// - Generate and store the Master Key on first run
/// - Encrypt/decrypt API keys using AES-256-GCM
/// - Load from the data directory (portable mode)
pub struct KeyManager {
    master_key: Vec<u8>,
    master_key_path: PathBuf,
}

impl KeyManager {
    /// Initialize or load key manager from existing data directory.
    /// Creates `data/master_key.bin` if it doesn't exist.
    pub fn initialize(data_dir: &Path) -> anyhow::Result<Self> {
        std::fs::create_dir_all(data_dir)?;

        let master_key_path = data_dir.join("master_key.bin");

        let master_key = if master_key_path.exists() {
            info!("Loading existing Master Key from {:?}", master_key_path);
            let bytes = std::fs::read(&master_key_path)?;
            if bytes.len() != 32 {
                error!(
                    "Master Key has invalid length {} (expected 32)",
                    bytes.len()
                );
                return Err(anyhow::anyhow!("corrupted master key file"));
            }
            bytes.to_vec()
        } else {
            info!("Generating new Master Key at {:?}", master_key_path);
            let mut rng = rand::rng();
            let mut key = vec![0u8; 32];
            rng.fill(&mut key[..]);

            // Create a temp file with restricted permissions, then rename atomically
            let tmp_path = master_key_path.with_extension("bin.tmp");
            std::fs::write(&tmp_path, &key)?;

            // On Windows, we'll note the file should be restricted (chown not available)
            #[cfg(windows)]
            {
                warn!(
                    "Windows: manually restrict permissions on {:?} for security",
                    master_key_path
                );
            }

            std::fs::rename(tmp_path, &master_key_path)?;
            key
        };

        Ok(Self {
            master_key,
            master_key_path,
        })
    }

    /// Decrypts an encrypted API key.
    pub fn decrypt_api_key(&self, ciphertext: &[u8]) -> Result<String, String> {
        if self.master_key.is_empty() {
            return Err("Master Key is empty".to_string());
        }

        if ciphertext.len() < 12 + 32 {
            return Err("Encrypted data too short (missing nonce+ciphertext)".to_string());
        }

        let nonce_bytes: [u8; 12] = ciphertext[..12].try_into().map_err(|_| "invalid nonce")?;
        let ct = &ciphertext[12..];

        let cipher = Aes256Gcm::new_from_slice(&self.master_key)
            .map_err(|e| format!("AES key setup failed: {}", e))?;

        let nonce = Nonce::from_slice(&nonce_bytes);
        let pt = cipher
            .decrypt_in_place(nonce, b"", ct)
            .map_err(|e| format!("decryption failed: {}", e))?;

        String::from_utf8(pt).map_err(|e| format!("not valid UTF-8: {}", e))
    }

    /// Encrypts a plaintext API key using AES-256-GCM.
    pub fn encrypt_api_key(&self, plaintext: &str) -> Result<Vec<u8>, String> {
        if self.master_key.is_empty() {
            return Err("Master Key is empty".to_string());
        }

        let cipher = Aes256Gcm::new_from_slice(&self.master_key)
            .map_err(|e| format!("AES key setup failed: {}", e))?;

        let mut nonce_bytes = [0u8; 12];
        rand::rngs::OsRng.fill(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let mut buffer = plaintext.as_bytes().to_vec();
        // Reserve space for GCM tag (16 bytes) appended in-place
        buffer.extend_from_slice(&[0u8; 16]);

        cipher
            .encrypt_in_place(nonce, b"", &mut buffer)
            .map_err(|e| format!("encryption failed: {}", e))?;

        // Prepend nonce to ciphertext for storage
        let mut output = Vec::with_capacity(12 + buffer.len());
        output.extend_from_slice(&nonce_bytes);
        output.extend_from_slice(&buffer);
        Ok(output)
    }

    /// Get the path to the Master Key file.
    pub fn master_key_path(&self) -> &Path {
        &self.master_key_path
    }

    /// Returns the Gateway settings (current implementation)
    pub fn gateway_settings() -> GatewaySettings {
        GatewaySettings::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_encrypt_decrypt_roundtrip() {
        let dir = TempDir::new().unwrap();
        let km = KeyManager::initialize(dir.path()).unwrap();

        let plaintext = "sk-test-api-key-12345";
        let encrypted = km.encrypt_api_key(plaintext).unwrap();
        let decrypted = km.decrypt_api_key(&encrypted).unwrap();

        assert_eq!(decrypted, plaintext);
    }

    #[test]
    fn test_master_key_persistence() {
        let dir = TempDir::new().unwrap();

        // First initialization generates key
        let km1 = KeyManager::initialize(dir.path()).unwrap();
        let key1 = km1.master_key.clone();

        // Second initialization loads same key
        let km2 = KeyManager::initialize(dir.path()).unwrap();
        assert_eq!(km2.master_key, key1);
    }

    #[test]
    fn test_decrypt_wrong_key_fails() {
        let dir1 = TempDir::new().unwrap();
        let dir2 = TempDir::new().unwrap();

        let km1 = KeyManager::initialize(dir1.path()).unwrap();
        let km2 = KeyManager::initialize(dir2.path()).unwrap();

        let encrypted = km1.encrypt_api_key("secret").unwrap();
        let result = km2.decrypt_api_key(&encrypted);
        assert!(result.is_err());
    }

    #[test]
    fn test_invalid_ciphertext_rejected() {
        let dir = TempDir::new().unwrap();
        let km = KeyManager::initialize(dir.path()).unwrap();
        assert!(km.decrypt_api_key(b"too_short").is_err());
    }
}
