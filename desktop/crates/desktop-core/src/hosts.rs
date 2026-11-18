// SPDX-License-Identifier: MIT OR Apache-2.0
//! The saved hosts: `hosts.json` in the config dir. Tokens are never
//! in this file; they live in the [`crate::TokenStore`], keyed by the
//! record's id.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// One agent or hub the user added.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostRecord {
    /// A random id, stable across renames; the token store's key.
    pub id: String,
    /// The name the user gave it.
    pub name: String,
    /// The address to connect to (a name or an IP literal).
    pub host: String,
    pub port: u16,
    /// The `SHA256:<hex>` pin; empty means plain HTTP, loopback only.
    pub pin: String,
}

impl HostRecord {
    /// A new record with a fresh random id.
    pub fn new(name: &str, host: &str, port: u16, pin: &str) -> Self {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).expect("the OS random source");
        Self {
            id: bytes.iter().map(|b| format!("{b:02x}")).collect(),
            name: name.to_string(),
            host: host.to_string(),
            port,
            pin: pin.to_string(),
        }
    }

    /// `host:port` as the client dials it; IPv6 literals get brackets.
    pub fn target(&self) -> String {
        if self.host.contains(':') && !self.host.starts_with('[') {
            format!("[{}]:{}", self.host, self.port)
        } else {
            format!("{}:{}", self.host, self.port)
        }
    }
}

/// The list of saved hosts and the file it lives in.
#[derive(Debug)]
pub struct HostRegistry {
    file: PathBuf,
    pub records: Vec<HostRecord>,
}

impl HostRegistry {
    /// Loads `dir/hosts.json`; a missing file is an empty fleet, and an
    /// unreadable one is an error rather than a silent wipe on save.
    pub fn load(dir: &Path) -> std::io::Result<Self> {
        let file = dir.join("hosts.json");
        let records = match std::fs::read_to_string(&file) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        Ok(Self { file, records })
    }

    /// Writes the list atomically (temp file, then rename), mode 0600.
    pub fn save(&self) -> std::io::Result<()> {
        if let Some(parent) = self.file.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = self.file.with_extension("json.tmp");
        {
            let mut out = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(&tmp)?;
            serde_json::to_writer_pretty(&mut out, &self.records)?;
            out.write_all(b"\n")?;
            out.sync_all()?;
        }
        std::fs::rename(&tmp, &self.file)
    }

    pub fn get(&self, id: &str) -> Option<&HostRecord> {
        self.records.iter().find(|r| r.id == id)
    }

    /// Adds a record and saves.
    pub fn add(&mut self, record: HostRecord) -> std::io::Result<()> {
        self.records.push(record);
        self.save()
    }

    /// Removes a record by id and saves; a missing id is not an error.
    pub fn remove(&mut self, id: &str) -> std::io::Result<()> {
        self.records.retain(|r| r.id != id);
        self.save()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ztop-desktop-{tag}-{}",
            HostRecord::new("x", "x", 1, "").id
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn records_round_trip_through_the_file_without_tokens() {
        let dir = temp_dir("hosts");
        let mut registry = HostRegistry::load(&dir).unwrap();
        assert!(registry.records.is_empty());
        let a = HostRecord::new("host-a", "127.0.0.1", 7100, "SHA256:00");
        let b = HostRecord::new("host-b", "::1", 7200, "");
        registry.add(a.clone()).unwrap();
        registry.add(b.clone()).unwrap();

        let again = HostRegistry::load(&dir).unwrap();
        assert_eq!(again.records, vec![a.clone(), b.clone()]);
        let text = std::fs::read_to_string(dir.join("hosts.json")).unwrap();
        assert!(!text.contains("token"));
        let mode = std::fs::metadata(dir.join("hosts.json"))
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o777, 0o600);

        let mut again = again;
        again.remove(&a.id).unwrap();
        assert_eq!(HostRegistry::load(&dir).unwrap().records, vec![b]);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn a_corrupt_file_is_an_error_not_an_empty_fleet() {
        let dir = temp_dir("corrupt");
        std::fs::write(dir.join("hosts.json"), "{not json").unwrap();
        assert!(HostRegistry::load(&dir).is_err());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ids_are_distinct_and_ipv6_targets_get_brackets() {
        let a = HostRecord::new("a", "::1", 7100, "");
        let b = HostRecord::new("b", "host-b.example.com", 7100, "");
        assert_ne!(a.id, b.id);
        assert_eq!(a.id.len(), 16);
        assert_eq!(a.target(), "[::1]:7100");
        assert_eq!(b.target(), "host-b.example.com:7100");
    }
}
