use std::collections::HashMap;

use aes_gcm::aead::{Aead, KeyInit, OsRng};
use aes_gcm::{AeadCore, Aes256Gcm, Nonce};
use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STD;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("invalid key material: {0}")]
    InvalidKey(String),
    #[error("unknown encryption key version: {0}")]
    UnknownKeyVersion(u32),
    #[error("invalid nonce length: expected 12 bytes, got {0}")]
    InvalidNonceLength(usize),
    #[error("encryption failed")]
    EncryptionFailed,
    #[error("decryption failed")]
    DecryptionFailed,
    #[error("serialization error: {0}")]
    Serialization(String),
    #[error("invalid compact envelope: {0}")]
    InvalidEnvelope(String),
}

pub type CryptoResult<T> = Result<T, CryptoError>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EncryptedEnvelope {
    pub key_version: u32,
    pub nonce: [u8; 12],
    pub ciphertext: Vec<u8>,
}

impl EncryptedEnvelope {
    pub fn to_compact_base64(&self) -> String {
        let mut raw = Vec::with_capacity(4 + 12 + self.ciphertext.len());
        raw.extend_from_slice(&self.key_version.to_be_bytes());
        raw.extend_from_slice(&self.nonce);
        raw.extend_from_slice(&self.ciphertext);
        BASE64_STD.encode(raw)
    }

    pub fn from_compact_base64(encoded: &str) -> CryptoResult<Self> {
        let raw = BASE64_STD
            .decode(encoded.trim())
            .map_err(|e| CryptoError::InvalidEnvelope(e.to_string()))?;
        if raw.len() < 16 {
            return Err(CryptoError::InvalidEnvelope(
                "envelope shorter than 16 bytes".to_string(),
            ));
        }
        let key_version = u32::from_be_bytes([raw[0], raw[1], raw[2], raw[3]]);
        let mut nonce = [0u8; 12];
        nonce.copy_from_slice(&raw[4..16]);
        let ciphertext = raw[16..].to_vec();
        Ok(Self {
            key_version,
            nonce,
            ciphertext,
        })
    }
}

#[derive(Clone)]
pub struct FieldCipher {
    current_version: u32,
    keys: HashMap<u32, [u8; 32]>,
}

impl std::fmt::Debug for FieldCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FieldCipher")
            .field("current_version", &self.current_version)
            .field("key_versions", &self.keys.keys().collect::<Vec<_>>())
            .finish()
    }
}

impl FieldCipher {
    pub fn from_single_key(key: [u8; 32]) -> Self {
        let mut keys = HashMap::new();
        keys.insert(1, key);
        Self {
            current_version: 1,
            keys,
        }
    }

    pub fn from_keyring(current_version: u32, keys: HashMap<u32, [u8; 32]>) -> CryptoResult<Self> {
        if !keys.contains_key(&current_version) {
            return Err(CryptoError::UnknownKeyVersion(current_version));
        }
        Ok(Self {
            current_version,
            keys,
        })
    }

    pub fn from_hex_or_base64(version: u32, secret: &str) -> CryptoResult<Self> {
        let key = decode_key_32(secret)?;
        let mut keys = HashMap::new();
        keys.insert(version, key);
        Self::from_keyring(version, keys)
    }

    /// Deterministic test/dev fallback key derived via SHA-256 of a passphrase.
    /// Production deployments MUST pass a high-entropy 32-byte key from KMS/env.
    pub fn from_passphrase_for_dev(passphrase: &str) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(b"medical-server-dev-key:");
        hasher.update(passphrase.as_bytes());
        let digest = hasher.finalize();
        let mut key = [0u8; 32];
        key.copy_from_slice(&digest);
        Self::from_single_key(key)
    }

    pub fn current_version(&self) -> u32 {
        self.current_version
    }

    pub fn encrypt_bytes(&self, plaintext: &[u8]) -> CryptoResult<EncryptedEnvelope> {
        let key = self
            .keys
            .get(&self.current_version)
            .ok_or(CryptoError::UnknownKeyVersion(self.current_version))?;
        let cipher = Aes256Gcm::new_from_slice(key)
            .map_err(|e| CryptoError::InvalidKey(e.to_string()))?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = cipher
            .encrypt(&nonce, plaintext)
            .map_err(|_| CryptoError::EncryptionFailed)?;
        let mut nonce_arr = [0u8; 12];
        nonce_arr.copy_from_slice(nonce.as_slice());
        Ok(EncryptedEnvelope {
            key_version: self.current_version,
            nonce: nonce_arr,
            ciphertext,
        })
    }

    pub fn decrypt_bytes(&self, envelope: &EncryptedEnvelope) -> CryptoResult<Vec<u8>> {
        let key = self
            .keys
            .get(&envelope.key_version)
            .ok_or(CryptoError::UnknownKeyVersion(envelope.key_version))?;
        let cipher = Aes256Gcm::new_from_slice(key)
            .map_err(|e| CryptoError::InvalidKey(e.to_string()))?;
        let nonce = Nonce::from_slice(&envelope.nonce);
        cipher
            .decrypt(nonce, envelope.ciphertext.as_ref())
            .map_err(|_| CryptoError::DecryptionFailed)
    }

    pub fn decrypt_parts(
        &self,
        key_version: u32,
        nonce: &[u8],
        ciphertext: &[u8],
    ) -> CryptoResult<Vec<u8>> {
        if nonce.len() != 12 {
            return Err(CryptoError::InvalidNonceLength(nonce.len()));
        }
        let mut nonce_arr = [0u8; 12];
        nonce_arr.copy_from_slice(nonce);
        self.decrypt_bytes(&EncryptedEnvelope {
            key_version,
            nonce: nonce_arr,
            ciphertext: ciphertext.to_vec(),
        })
    }

    pub fn encrypt_json<T: Serialize>(&self, value: &T) -> CryptoResult<EncryptedEnvelope> {
        let bytes =
            serde_json::to_vec(value).map_err(|e| CryptoError::Serialization(e.to_string()))?;
        self.encrypt_bytes(&bytes)
    }

    pub fn decrypt_json<T: DeserializeOwned>(&self, envelope: &EncryptedEnvelope) -> CryptoResult<T> {
        let bytes = self.decrypt_bytes(envelope)?;
        serde_json::from_slice(&bytes).map_err(|e| CryptoError::Serialization(e.to_string()))
    }
}

pub fn decode_key_32(secret: &str) -> CryptoResult<[u8; 32]> {
    let trimmed = secret.trim();
    if trimmed.len() == 64 && trimmed.chars().all(|c| c.is_ascii_hexdigit()) {
        let bytes = hex::decode(trimmed).map_err(|e| CryptoError::InvalidKey(e.to_string()))?;
        let mut out = [0u8; 32];
        out.copy_from_slice(&bytes);
        return Ok(out);
    }
    let bytes = BASE64_STD
        .decode(trimmed)
        .map_err(|e| CryptoError::InvalidKey(e.to_string()))?;
    if bytes.len() != 32 {
        return Err(CryptoError::InvalidKey(format!(
            "expected 32-byte key, got {} bytes",
            bytes.len()
        )));
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes);
    Ok(out)
}

/// Computes an anonymized salted SHA-256 patient identifier (`pat_sha256_<hex>`).
pub fn hash_patient_id(raw_id: &str, salt: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(salt);
    hasher.update(b":");
    hasher.update(raw_id.trim().as_bytes());
    let digest = hasher.finalize();
    format!("pat_sha256_{}", hex::encode(digest))
}

/// Computes SHA-256 hex digest for API key persistence (`api_keys.key_hash`).
pub fn hash_api_key(raw_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(raw_key.as_bytes());
    hex::encode(hasher.finalize())
}
