// SPDX-License-Identifier: MIT OR Apache-2.0
//! Pairing and tokens: how a client earns access to an agent.
//!
//! The flow is DESIGN.md's: the agent shows a short code, the client
//! sends it once over the paired connection, and it receives a
//! long-lived token. Only the token's SHA-256 hash is stored, so a
//! copy of the agent's files cannot be replayed as a token. Codes are
//! single-use and expire, and a few wrong guesses burn them; tokens can
//! be listed and revoked by a token holding the `manage` right.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

/// How long a pairing code stays valid.
pub const PAIRING_TTL: Duration = Duration::from_secs(5 * 60);

/// Unambiguous symbols: no `0`, `1`, `i`, `l` or `o`.
const CODE_ALPHABET: &[u8] = b"abcdefghjkmnpqrstuvwxyz23456789";

/// Symbols per code: 31^8, about 8.5 * 10^11 codes.
pub const CODE_LENGTH: usize = 8;

/// Wrong guesses one code survives: the next wrong guess after these
/// burns it, so a guessing client gets five tries per printed code.
pub const MAX_WRONG_GUESSES: u32 = 5;

/// The least time between a wrong guess and the next attempt; an
/// attempt sooner is refused without being counted.
pub const GUESS_SPACING: Duration = Duration::from_secs(1);

/// The right to list and revoke tokens.
pub const RIGHT_MANAGE: &str = "manage";

/// Why pairing or a token check failed.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// No pairing code is pending, it does not match, or it expired.
    #[error("the pairing code does not match, is used up or expired")]
    BadCode,
    /// A guess came too soon after a wrong one; try again in a second.
    #[error("too many pairing attempts: wait a second and try again")]
    TooSoon,
    /// The tokens file could not be read or written.
    #[error("the tokens file failed: {0}")]
    Io(#[from] std::io::Error),
}

/// One accepted client.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TokenRecord {
    /// The token's full SHA-256 (hex). This is what the map is keyed
    /// by and what the file stores; the token itself is never kept.
    pub hash: String,
    /// Eight hex characters of the hash: identifies the token in the
    /// list and the revoke route without widening the surface.
    pub id: String,
    /// What the pairing was for, shown in the list.
    pub label: String,
    /// When the token was created (unix ms).
    pub created_ms: u64,
    /// What this token may do: `"read"` always, plus `"control"` when
    /// the agent pairs with control allowed and `"manage"` (list and
    /// revoke tokens) when it pairs with management allowed. Records
    /// saved before the rights existed read as read-only.
    #[serde(default = "default_rights")]
    pub rights: Vec<String>,
}

/// The rights every token has at least.
fn default_rights() -> Vec<String> {
    vec!["read".to_string()]
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct Stored {
    tokens: Vec<TokenRecord>,
}

/// A pending pairing code: what to type, and when it dies.
struct Pending {
    code: String,
    label: String,
    expires_ms: u64,
    rights: Vec<String>,
    /// Wrong guesses so far against this code.
    wrong: u32,
}

/// The pairing and token state behind the agent's auth.
#[derive(Default)]
pub struct Auth {
    /// Where tokens persist; `None` keeps them in memory only (tests,
    /// or a host with no home directory).
    path: Option<PathBuf>,
    /// Token hash (hex) to record.
    tokens: HashMap<String, TokenRecord>,
    /// The one pending pairing code, if any.
    pending: Option<Pending>,
    /// When the last wrong guess came (unix ms), for the spacing rule.
    last_wrong_ms: Option<u64>,
}

impl Auth {
    /// An in-memory auth: nothing persists.
    pub fn in_memory() -> Self {
        Self::default()
    }

    /// Auth backed by a tokens file (loaded if it exists).
    pub fn at(path: PathBuf) -> Result<Self, AuthError> {
        let mut auth = Self {
            path: Some(path),
            tokens: HashMap::new(),
            pending: None,
            last_wrong_ms: None,
        };
        auth.reload()?;
        Ok(auth)
    }

    /// Re-read the tokens file, ignoring a missing file.
    pub fn reload(&mut self) -> Result<(), AuthError> {
        self.tokens.clear();
        let Some(path) = &self.path else {
            return Ok(());
        };
        let Ok(text) = std::fs::read_to_string(path) else {
            return Ok(());
        };
        let stored: Stored = serde_json::from_str(&text).map_err(|_| {
            AuthError::Io(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "unreadable tokens file",
            ))
        })?;
        for record in stored.tokens {
            self.tokens.insert(record.hash.clone(), record);
        }
        Ok(())
    }

    /// Write the token hashes to disk, best effort: the agent serves
    /// even when the file cannot be written, and reports on stderr.
    pub fn save(&self) {
        let Some(path) = &self.path else {
            return;
        };
        let mut records: Vec<&TokenRecord> = self.tokens.values().collect();
        records.sort_by(|a, b| a.id.cmp(&b.id));
        let stored = Stored {
            tokens: records.into_iter().cloned().collect(),
        };
        let text = match serde_json::to_string_pretty(&stored) {
            Ok(text) => text,
            Err(_) => return,
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        // 0600 on unix: the file holds credentials' hashes, and only
        // the agent's user should read them.
        #[cfg(unix)]
        let write = {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(true)
                .mode(0o600)
                .open(path)
                .and_then(|mut file| file.write_all(text.as_bytes()))
        };
        #[cfg(not(unix))]
        let write = std::fs::write(path, text);
        if let Err(error) = write {
            eprintln!("ztop-agent: could not save tokens: {error}");
        }
    }

    /// Start a pairing: return the code to show the person at the
    /// machine. One code at a time — a new pairing replaces the old.
    pub fn start_pairing(
        &mut self,
        label: String,
        now_ms: u64,
        rights: Vec<String>,
    ) -> Result<String, AuthError> {
        // Rejection sampling: only bytes below the largest multiple of
        // the alphabet's size are used, so every symbol is equally
        // likely (a plain modulo favors the first 256 % 31 symbols).
        let limit = (256 / CODE_ALPHABET.len() * CODE_ALPHABET.len()) as u8;
        let mut code = String::with_capacity(CODE_LENGTH);
        while code.len() < CODE_LENGTH {
            let mut bytes = [0u8; 32];
            getrandom::fill(&mut bytes).map_err(|_| {
                AuthError::Io(std::io::Error::other("no randomness for the pairing code"))
            })?;
            for byte in bytes.into_iter().filter(|byte| *byte < limit) {
                if code.len() == CODE_LENGTH {
                    break;
                }
                code.push(CODE_ALPHABET[byte as usize % CODE_ALPHABET.len()] as char);
            }
        }
        self.pending = Some(Pending {
            code,
            label,
            expires_ms: now_ms + PAIRING_TTL.as_millis() as u64,
            rights,
            wrong: 0,
        });
        self.last_wrong_ms = None;
        Ok(self.pending.as_ref().expect("just set").code.clone())
    }

    /// Trade a pairing code for a token. The code is single-use and
    /// must not be expired; the raw token is returned exactly once and
    /// only its hash is kept.
    ///
    /// A guess within [`GUESS_SPACING`] of a wrong one is refused with
    /// [`AuthError::TooSoon`] and not counted; the
    /// [`MAX_WRONG_GUESSES`]th wrong guess burns the code, and the
    /// person at the machine starts a new pairing.
    pub fn pair(&mut self, code: &str, now_ms: u64) -> Result<String, AuthError> {
        if let Some(last) = self.last_wrong_ms {
            if now_ms < last.saturating_add(GUESS_SPACING.as_millis() as u64) {
                return Err(AuthError::TooSoon);
            }
        }
        let matches = self.pending.as_ref().is_some_and(|pending| {
            pending.expires_ms > now_ms && same_code(pending.code.as_bytes(), code.as_bytes())
        });
        if !matches {
            self.last_wrong_ms = Some(now_ms);
            if let Some(pending) = &mut self.pending {
                pending.wrong += 1;
                if pending.wrong >= MAX_WRONG_GUESSES || pending.expires_ms <= now_ms {
                    self.pending = None;
                }
            }
            return Err(AuthError::BadCode);
        }
        let pending = self.pending.take().expect("matched a pending code");
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes)
            .map_err(|_| AuthError::Io(std::io::Error::other("no randomness for the token")))?;
        let token = hex(&bytes);
        let hash = hex(&Sha256::digest(token.as_bytes()));
        let record = TokenRecord {
            hash: hash.clone(),
            id: hash[..8].to_string(),
            label: pending.label,
            created_ms: now_ms,
            rights: pending.rights,
        };
        self.tokens.insert(hash, record);
        self.save();
        Ok(token)
    }

    /// The pending pairing code, if one is live. Read-only: the person
    /// at the machine is the one who turns it into a token.
    pub fn pending_code(&self) -> Option<String> {
        self.pending.as_ref().map(|pending| pending.code.clone())
    }

    /// Does this bearer token belong to an accepted client?
    pub fn check(&self, token: &str) -> bool {
        self.tokens
            .contains_key(&hex(&Sha256::digest(token.as_bytes())))
    }

    /// Does this bearer token carry the `control` right?
    pub fn can_control(&self, token: &str) -> bool {
        self.has_right(token, "control")
    }

    /// Does this bearer token carry the `manage` right (list and revoke
    /// tokens)?
    pub fn can_manage(&self, token: &str) -> bool {
        self.has_right(token, RIGHT_MANAGE)
    }

    /// Does this bearer token carry `right`?
    fn has_right(&self, token: &str, right: &str) -> bool {
        self.record_of(token)
            .is_some_and(|record| record.rights.iter().any(|held| held == right))
    }

    /// The record a bearer token belongs to.
    fn record_of(&self, token: &str) -> Option<&TokenRecord> {
        self.tokens.get(&hex(&Sha256::digest(token.as_bytes())))
    }

    /// The accepted clients, oldest first.
    pub fn list(&self) -> Vec<TokenRecord> {
        let mut records: Vec<TokenRecord> = self.tokens.values().cloned().collect();
        records.sort_by(|a, b| a.created_ms.cmp(&b.created_ms).then(a.id.cmp(&b.id)));
        records
    }

    /// Revoke a token by id; true when one was removed.
    pub fn revoke(&mut self, id: &str) -> bool {
        let removed = self
            .tokens
            .values()
            .find(|record| record.id == id)
            .map(|record| record.hash.clone());
        match removed {
            Some(hash) => {
                self.tokens.remove(&hash);
                self.save();
                true
            }
            None => false,
        }
    }
}

/// Compare two codes without stopping at the first differing byte, so
/// the time a refusal takes says nothing about how much of a guess was
/// right.
fn same_code(expected: &[u8], guess: &[u8]) -> bool {
    if expected.len() != guess.len() {
        return false;
    }
    expected
        .iter()
        .zip(guess)
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

/// Lowercase hex, std-only.
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// The current unix time in milliseconds.
pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 1_700_000_000_000;

    #[test]
    fn pairing_issues_a_single_use_token() {
        let mut auth = Auth::in_memory();
        let code = auth
            .start_pairing("the app".into(), NOW, vec!["read".to_string()])
            .expect("code");
        assert_eq!(code.len(), CODE_LENGTH);
        assert!(!code.contains('0') && !code.contains('l'));

        let token = auth.pair(&code, NOW + 1_000).expect("token");
        assert_eq!(token.len(), 64);
        assert!(auth.check(&token));
        assert_eq!(auth.list().len(), 1);
        assert_eq!(auth.list()[0].label, "the app");

        // The code is spent: the same code pairs no second client.
        assert!(matches!(
            auth.pair(&code, NOW + 2_000),
            Err(AuthError::BadCode)
        ));
    }

    #[test]
    fn a_wrong_or_stale_code_is_refused() {
        let mut auth = Auth::in_memory();
        auth.start_pairing("the app".into(), NOW, vec!["read".to_string()])
            .expect("code");
        assert!(matches!(auth.pair("wrong!", NOW), Err(AuthError::BadCode)));
        // Expired: five minutes and one millisecond.
        let code = auth.pending_code().expect("pending");
        assert!(matches!(
            auth.pair(&code, NOW + PAIRING_TTL.as_millis() as u64 + 1),
            Err(AuthError::BadCode)
        ));
        assert!(auth.list().is_empty());
    }

    /// Regression: a wrong guess left the code valid and nothing
    /// limited the rate, so the code space (six symbols of 31, about
    /// 887 million) fell to a guessing client in hours. Five wrong
    /// guesses now burn the code, guesses are spaced, and codes are
    /// eight symbols long.
    #[test]
    fn wrong_guesses_burn_the_code() {
        let mut auth = Auth::in_memory();
        let code = auth
            .start_pairing("the app".into(), NOW, vec!["read".to_string()])
            .expect("code");
        let mut at = NOW;
        for _ in 0..MAX_WRONG_GUESSES {
            assert!(matches!(
                auth.pair("wrongcode", at),
                Err(AuthError::BadCode)
            ));
            at += GUESS_SPACING.as_millis() as u64;
        }
        assert!(auth.pending_code().is_none(), "the code must be burned");
        assert!(matches!(auth.pair(&code, at), Err(AuthError::BadCode)));
        assert!(auth.list().is_empty());
    }

    #[test]
    fn guesses_right_after_a_wrong_one_wait() {
        let mut auth = Auth::in_memory();
        let code = auth
            .start_pairing("the app".into(), NOW, vec!["read".to_string()])
            .expect("code");
        assert!(matches!(
            auth.pair("wrongcode", NOW),
            Err(AuthError::BadCode)
        ));
        // Too soon: refused without counting, and the code survives.
        assert!(matches!(
            auth.pair(&code, NOW + 10),
            Err(AuthError::TooSoon)
        ));
        let later = NOW + GUESS_SPACING.as_millis() as u64;
        assert!(auth.pair(&code, later).is_ok());
    }

    #[test]
    fn codes_are_long_and_drawn_from_the_alphabet() {
        let mut auth = Auth::in_memory();
        for _ in 0..200 {
            let code = auth
                .start_pairing("the app".into(), NOW, vec!["read".to_string()])
                .expect("code");
            assert_eq!(code.len(), CODE_LENGTH);
            assert!(
                code.bytes().all(|byte| CODE_ALPHABET.contains(&byte)),
                "{code}"
            );
        }
    }

    #[test]
    fn a_replaced_pairing_kills_the_older_code() {
        let mut auth = Auth::in_memory();
        let first = auth
            .start_pairing("first".into(), NOW, vec!["read".to_string()])
            .expect("code");
        auth.start_pairing("second".into(), NOW, vec!["read".to_string()])
            .expect("code");
        assert!(matches!(auth.pair(&first, NOW), Err(AuthError::BadCode)));
        assert_eq!(auth.list().len(), 0);
    }

    #[test]
    fn tokens_persist_as_hashes_and_can_be_revoked() {
        let dir = std::env::temp_dir().join("ztop-auth-test-tokens");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        let path = dir.join("tokens.json");

        let token = {
            let mut auth = Auth::at(path.clone()).expect("auth");
            auth.start_pairing("the app".into(), NOW, vec!["read".to_string()])
                .expect("code");
            let code = auth.pending_code().expect("pending");
            auth.pair(&code, NOW).expect("token")
        };

        // A fresh reader knows the client by hash only.
        let mut auth = Auth::at(path.clone()).expect("auth");
        assert!(auth.check(&token));
        let records = auth.list();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].id.len(), 8);
        let id = records[0].id.clone();

        assert!(auth.revoke(&id));
        assert!(!auth.check(&token));
        assert!(!auth.revoke(&id));

        let text = std::fs::read_to_string(&path).expect("file");
        assert!(!text.contains(&token), "the raw token must never be stored");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_check_rejects_arbitrary_strings() {
        let auth = Auth::in_memory();
        assert!(!auth.check(""));
        assert!(!auth.check(&"x".repeat(64)));
    }
}
