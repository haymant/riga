use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, AeadCore, KeyInit, OsRng},
};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

#[derive(Clone)]
pub struct SecureStore {
    root: PathBuf,
    key: [u8; 32],
}

impl SecureStore {
    pub fn new(root: PathBuf, token: &str) -> Self {
        let mut key = [0_u8; 32];
        key.copy_from_slice(&Sha256::digest(token.as_bytes()));
        Self { root, key }
    }

    pub fn from_env() -> Option<Self> {
        let token = std::env::var("RIGA_TOKEN")
            .ok()
            .filter(|value| !value.is_empty())?;
        let root = std::env::var_os("RIGA_DATA_DIR")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share/riga"))
            })
            .unwrap_or_else(|| PathBuf::from(".riga-data"));
        Some(Self::new(root, &token))
    }

    pub fn load<T: serde::de::DeserializeOwned>(&self, name: &str) -> Result<Option<T>, String> {
        let path = self.root.join(format!("{name}.enc"));
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        if bytes.len() < 12 {
            return Err("encrypted state is truncated".into());
        }
        let cipher = Aes256Gcm::new_from_slice(&self.key).map_err(|e| e.to_string())?;
        let plaintext = cipher
            .decrypt(Nonce::from_slice(&bytes[..12]), &bytes[12..])
            .map_err(|_| {
                "encrypted state cannot be decrypted with the current RIGA_TOKEN".to_owned()
            })?;
        serde_json::from_slice(&plaintext)
            .map(Some)
            .map_err(|e| e.to_string())
    }

    pub fn save<T: serde::Serialize>(&self, name: &str, value: &T) -> Result<(), String> {
        fs::create_dir_all(&self.root).map_err(|e| e.to_string())?;
        let cipher = Aes256Gcm::new_from_slice(&self.key).map_err(|e| e.to_string())?;
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let ciphertext = cipher
            .encrypt(
                &nonce,
                serde_json::to_vec(value)
                    .map_err(|e| e.to_string())?
                    .as_ref(),
            )
            .map_err(|_| "unable to encrypt state".to_owned())?;
        let mut bytes = nonce.to_vec();
        bytes.extend(ciphertext);
        let temp = self.root.join(format!(".{name}.tmp"));
        let path = self.root.join(format!("{name}.enc"));
        fs::write(&temp, bytes).map_err(|e| e.to_string())?;
        fs::rename(temp, path).map_err(|e| e.to_string())
    }
}

pub fn database_backend() -> &'static str {
    match std::env::var("DATABASE_URL").as_deref() {
        Ok(url) if url.starts_with("sqlite:") => "sqlite",
        Ok(url) if url.starts_with("postgres:") || url.starts_with("postgresql:") => "postgres",
        Ok(_) => "unsupported",
        Err(_) => "encrypted-file",
    }
}

#[cfg(test)]
mod tests {
    use super::SecureStore;
    use tempfile::tempdir;

    #[test]
    fn encrypted_round_trip_does_not_store_plaintext() {
        let directory = tempdir().unwrap();
        let store = SecureStore::new(directory.path().to_owned(), "test-token");
        store
            .save("test", &serde_json::json!({"secret":"value"}))
            .unwrap();
        let bytes = std::fs::read(directory.path().join("test.enc")).unwrap();
        assert!(!String::from_utf8_lossy(&bytes).contains("value"));
        assert_eq!(
            store.load::<serde_json::Value>("test").unwrap().unwrap()["secret"],
            "value"
        );
    }
}
