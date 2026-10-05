use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, AeadCore, KeyInit, OsRng},
};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf};

use sqlx::Row;

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

pub(crate) fn data_root() -> PathBuf {
    std::env::var_os("RIGA_DATA_DIR")
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/share/riga"))
        })
        .unwrap_or_else(|| PathBuf::from(".riga-data"))
}

/// Loads application state from the configured database, encrypted file store, or a local
/// JSON file when no encryption token has been configured yet.
pub async fn load_json<T: serde::de::DeserializeOwned>(name: &str) -> Result<Option<T>, String> {
    if let Some(url) = std::env::var("DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        return load_database(&url, name).await;
    }
    if let Some(store) = SecureStore::from_env() {
        return store.load(name);
    }
    let path = data_root().join(format!("{name}.json"));
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| error.to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

/// Saves application state to the configured database, encrypted file store, or a local
/// JSON file when no encryption token has been configured yet.
pub async fn save_json<T: serde::Serialize>(name: &str, value: &T) -> Result<(), String> {
    if let Some(url) = std::env::var("DATABASE_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
    {
        return save_database(&url, name, value).await;
    }
    if let Some(store) = SecureStore::from_env() {
        return store.save(name, value);
    }
    let root = data_root();
    fs::create_dir_all(&root).map_err(|error| error.to_string())?;
    let temp = root.join(format!(".{name}.tmp"));
    let path = root.join(format!("{name}.json"));
    fs::write(
        &temp,
        serde_json::to_vec_pretty(value).map_err(|error| error.to_string())?,
    )
    .map_err(|error| error.to_string())?;
    fs::rename(temp, path).map_err(|error| error.to_string())
}

async fn load_database<T: serde::de::DeserializeOwned>(
    url: &str,
    name: &str,
) -> Result<Option<T>, String> {
    match database_backend() {
        "sqlite" => {
            let pool = sqlx::SqlitePool::connect(url)
                .await
                .map_err(|error| error.to_string())?;
            sqlx::query("CREATE TABLE IF NOT EXISTS riga_settings (name TEXT PRIMARY KEY, value TEXT NOT NULL)")
                .execute(&pool)
                .await
                .map_err(|error| error.to_string())?;
            let row = sqlx::query("SELECT value FROM riga_settings WHERE name = ?")
                .bind(name)
                .fetch_optional(&pool)
                .await
                .map_err(|error| error.to_string())?;
            row.map(|value| {
                value
                    .try_get::<String, _>("value")
                    .map_err(|error| error.to_string())
            })
            .transpose()?
            .map(|value| serde_json::from_str(&value).map_err(|error| error.to_string()))
            .transpose()
        }
        "postgres" => {
            let pool = sqlx::PgPool::connect(url)
                .await
                .map_err(|error| error.to_string())?;
            sqlx::query("CREATE TABLE IF NOT EXISTS riga_settings (name TEXT PRIMARY KEY, value TEXT NOT NULL)")
                .execute(&pool)
                .await
                .map_err(|error| error.to_string())?;
            let row = sqlx::query("SELECT value FROM riga_settings WHERE name = $1")
                .bind(name)
                .fetch_optional(&pool)
                .await
                .map_err(|error| error.to_string())?;
            row.map(|value| {
                value
                    .try_get::<String, _>("value")
                    .map_err(|error| error.to_string())
            })
            .transpose()?
            .map(|value| serde_json::from_str(&value).map_err(|error| error.to_string()))
            .transpose()
        }
        _ => Err("DATABASE_URL must use sqlite:, postgres:, or postgresql:".into()),
    }
}

async fn save_database<T: serde::Serialize>(
    url: &str,
    name: &str,
    value: &T,
) -> Result<(), String> {
    let serialized = serde_json::to_string(value).map_err(|error| error.to_string())?;
    match database_backend() {
        "sqlite" => {
            let pool = sqlx::SqlitePool::connect(url)
                .await
                .map_err(|error| error.to_string())?;
            sqlx::query("CREATE TABLE IF NOT EXISTS riga_settings (name TEXT PRIMARY KEY, value TEXT NOT NULL)")
                .execute(&pool)
                .await
                .map_err(|error| error.to_string())?;
            sqlx::query("INSERT INTO riga_settings (name, value) VALUES (?, ?) ON CONFLICT(name) DO UPDATE SET value = excluded.value")
                .bind(name)
                .bind(serialized)
                .execute(&pool)
                .await
                .map_err(|error| error.to_string())?;
        }
        "postgres" => {
            let pool = sqlx::PgPool::connect(url)
                .await
                .map_err(|error| error.to_string())?;
            sqlx::query("CREATE TABLE IF NOT EXISTS riga_settings (name TEXT PRIMARY KEY, value TEXT NOT NULL)")
                .execute(&pool)
                .await
                .map_err(|error| error.to_string())?;
            sqlx::query("INSERT INTO riga_settings (name, value) VALUES ($1, $2) ON CONFLICT(name) DO UPDATE SET value = EXCLUDED.value")
                .bind(name)
                .bind(serialized)
                .execute(&pool)
                .await
                .map_err(|error| error.to_string())?;
        }
        _ => return Err("DATABASE_URL must use sqlite:, postgres:, or postgresql:".into()),
    }
    Ok(())
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
