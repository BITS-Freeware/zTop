// SPDX-License-Identifier: MIT OR Apache-2.0
//! Tokens in the desktop's Secret Service (GNOME Keyring, KWallet)
//! through libsecret: one item per host in the default collection,
//! found by the schema `dev.ztop.Desktop.Token` and its attribute
//! `host-id=<record id>`. libsecret's sync calls run only on worker
//! threads; each call builds its own schema value.

use std::collections::HashMap;

use gtk::gio;
use libsecret::{Schema, SchemaAttributeType, SchemaFlags};
use ztop_desktop_core::TokenStore;

const SCHEMA: &str = "dev.ztop.Desktop.Token";
const ATTRIBUTE: &str = "host-id";
/// The id used to ask whether the keyring answers at all.
const PROBE_ID: &str = "0000000000000000";

fn schema() -> Schema {
    Schema::new(
        SCHEMA,
        SchemaFlags::NONE,
        HashMap::from([(ATTRIBUTE, SchemaAttributeType::String)]),
    )
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SecretServiceStore;

impl SecretServiceStore {
    /// The store when a Secret Service answers a lookup; `None` sends
    /// the app to the file fallback.
    pub fn probe() -> Option<Self> {
        libsecret::password_lookup_sync(
            Some(&schema()),
            HashMap::from([(ATTRIBUTE, PROBE_ID)]),
            gio::Cancellable::NONE,
        )
        .ok()
        .map(|_| Self)
    }
}

impl TokenStore for SecretServiceStore {
    fn save(&self, id: &str, token: &str) -> Result<(), String> {
        libsecret::password_store_sync(
            Some(&schema()),
            HashMap::from([(ATTRIBUTE, id)]),
            Some(libsecret::COLLECTION_DEFAULT),
            &format!("zTop pairing token ({id})"),
            token,
            gio::Cancellable::NONE,
        )
        .map_err(|e| e.to_string())
    }

    fn load(&self, id: &str) -> Option<String> {
        libsecret::password_lookup_sync(
            Some(&schema()),
            HashMap::from([(ATTRIBUTE, id)]),
            gio::Cancellable::NONE,
        )
        .ok()
        .flatten()
        .map(|t| t.to_string())
        .filter(|t| !t.is_empty())
    }

    fn delete(&self, id: &str) {
        let _ = libsecret::password_clear_sync(
            Some(&schema()),
            HashMap::from([(ATTRIBUTE, id)]),
            gio::Cancellable::NONE,
        );
    }

    fn kind(&self) -> &'static str {
        "the desktop keyring"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Runs against a real Secret Service. CI starts one (dbus-run-session
    /// plus an unlocked gnome-keyring) and sets `ZTOP_TEST_KEYRING=1`;
    /// elsewhere the test says so and passes.
    #[test]
    fn tokens_round_trip_through_the_keyring() {
        if std::env::var("ZTOP_TEST_KEYRING").as_deref() != Ok("1") {
            eprintln!(
                "skipping: set ZTOP_TEST_KEYRING=1 inside a session with an unlocked keyring"
            );
            return;
        }
        let store = SecretServiceStore::probe()
            .expect("a Secret Service with an unlocked default collection");
        let (a, b) = ("00aa00aa00aa00aa", "00bb00bb00bb00bb");
        store.delete(a);
        store.delete(b);
        assert_eq!(store.load(a), None);
        store.save(a, "token-one").unwrap();
        store.save(a, "token-two").unwrap();
        store.save(b, "token-b").unwrap();
        assert_eq!(store.load(a).as_deref(), Some("token-two"));
        assert_eq!(store.load(b).as_deref(), Some("token-b"));
        store.delete(a);
        assert_eq!(store.load(a), None);
        assert_eq!(store.load(b).as_deref(), Some("token-b"));
        store.delete(b);
    }
}
