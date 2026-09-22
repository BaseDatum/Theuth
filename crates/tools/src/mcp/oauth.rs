//! `<data_dir>/oauth/mcp/<server>.json`: rmcp's credentials for one server,
//! mode 0600 (design doc 05 §3.3).

use std::path::PathBuf;

use rmcp::transport::auth::{AuthError, CredentialStore, StoredCredentials};

/// One JSON file. rmcp loads it on connect, saves after exchange/refresh,
/// clears on a definitive refresh rejection.
pub(super) struct FileStore {
    path: PathBuf,
}

impl FileStore {
    pub(super) fn new(path: PathBuf) -> FileStore {
        FileStore { path }
    }
}

fn store_err(e: impl std::fmt::Display) -> AuthError {
    AuthError::CredentialStoreError(e.to_string())
}

#[async_trait::async_trait]
impl CredentialStore for FileStore {
    async fn load(&self) -> Result<Option<StoredCredentials>, AuthError> {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => serde_json::from_str(&text).map(Some).map_err(store_err),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(store_err(e)),
        }
    }

    /// Write-then-rename, owner-only ([`platform::files::write_private`]).
    async fn save(&self, credentials: StoredCredentials) -> Result<(), AuthError> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).map_err(store_err)?;
        }
        let json = serde_json::to_string_pretty(&credentials).map_err(store_err)?;
        platform::files::write_private(&self.path, json.as_bytes()).map_err(store_err)
    }

    async fn clear(&self) -> Result<(), AuthError> {
        match std::fs::remove_file(&self.path) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(store_err(e)),
            _ => Ok(()),
        }
    }
}
