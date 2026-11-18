// SPDX-License-Identifier: MIT OR Apache-2.0
//! Where pairing tokens live. The app prefers the desktop's Secret
//! Service (GNOME Keyring, KWallet); [`FileTokenStore`] is the fallback
//! when none is running, one 0600 file per host in a 0700 directory.

use std::io::Write;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::path::PathBuf;

/// A place to keep one bearer token per host record id.
pub trait TokenStore: Send + Sync {
    fn save(&self, id: &str, token: &str) -> Result<(), String>;
    /// The token for `id`, or `None` when the host was never paired.
    fn load(&self, id: &str) -> Option<String>;
    /// Removes a token; a missing one is not an error.
    fn delete(&self, id: &str);
    /// A short name for where tokens go, shown in the app's About.
    fn kind(&self) -> &'static str;
}

/// Tokens as private files under `dir/tokens/`.
#[derive(Debug, Clone)]
pub struct FileTokenStore {
    dir: PathBuf,
}

impl FileTokenStore {
    pub fn new(config_dir: &std::path::Path) -> Self {
        Self {
            dir: config_dir.join("tokens"),
        }
    }

    fn file(&self, id: &str) -> Option<PathBuf> {
        // Ids are hex the app minted; anything else is refused rather
        // than turned into a path.
        if id.is_empty() || !id.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        Some(self.dir.join(id))
    }
}

impl TokenStore for FileTokenStore {
    fn save(&self, id: &str, token: &str) -> Result<(), String> {
        let file = self.file(id).ok_or("not a host id")?;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&self.dir)
            .map_err(|e| e.to_string())?;
        let mut out = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(file)
            .map_err(|e| e.to_string())?;
        out.write_all(token.as_bytes()).map_err(|e| e.to_string())
    }

    fn load(&self, id: &str) -> Option<String> {
        let text = std::fs::read_to_string(self.file(id)?).ok()?;
        let token = text.trim();
        (!token.is_empty()).then(|| token.to_string())
    }

    fn delete(&self, id: &str) {
        if let Some(file) = self.file(id) {
            let _ = std::fs::remove_file(file);
        }
    }

    fn kind(&self) -> &'static str {
        "a private file"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn file_tokens_round_trip_privately_and_refuse_odd_ids() {
        let dir = std::env::temp_dir().join(format!(
            "ztop-desktop-tokens-{}",
            crate::HostRecord::new("x", "x", 1, "").id
        ));
        let store = FileTokenStore::new(&dir);
        assert_eq!(store.load("00ab"), None);
        store.save("00ab", "token-one").unwrap();
        store.save("00ab", "token-two").unwrap();
        store.save("00cd", "token-b").unwrap();
        assert_eq!(store.load("00ab").as_deref(), Some("token-two"));
        assert_eq!(store.load("00cd").as_deref(), Some("token-b"));

        let file_mode = std::fs::metadata(dir.join("tokens/00ab"))
            .unwrap()
            .permissions()
            .mode();
        let dir_mode = std::fs::metadata(dir.join("tokens"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(file_mode & 0o777, 0o600);
        assert_eq!(dir_mode & 0o777, 0o700);

        assert!(store.save("../escape", "x").is_err());
        assert_eq!(store.load("../escape"), None);

        store.delete("00ab");
        store.delete("00ab");
        assert_eq!(store.load("00ab"), None);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
