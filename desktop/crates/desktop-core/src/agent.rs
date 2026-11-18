// SPDX-License-Identifier: MIT OR Apache-2.0
//! Talking to an agent or hub, through `ztop-core`'s client: one
//! blocking read or pairing per call. The app runs these on worker
//! threads, never on the UI thread.

use std::fmt;

use ztop_core::client::{RemoteCollector, pair_with, parse_pin};
use ztop_core::collector::{CollectError, Collector};
use ztop_core::model::Sample;

use crate::HostRecord;

/// Why a read failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReadError {
    /// The agent refused the token (401) or the host was never
    /// paired: the fix is pairing, so the card offers it.
    NeedsPairing,
    /// Anything else, in the client's own wording.
    Failed(String),
}

impl fmt::Display for ReadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NeedsPairing => f.write_str("the agent wants a paired token: pair with its code"),
            Self::Failed(why) => f.write_str(why),
        }
    }
}

/// Reads one snapshot from `record`, with `token` when it has one.
pub fn read(record: &HostRecord, token: Option<String>) -> Result<Sample, ReadError> {
    let mut collector = if record.pin.is_empty() {
        RemoteCollector::new(record.target())
    } else {
        if token.is_none() {
            // TLS means tokens are enforced (decision 0039): asking
            // without one only earns the same 401.
            return Err(ReadError::NeedsPairing);
        }
        RemoteCollector::tls(record.target(), &record.pin, token)
            .map_err(|e| ReadError::Failed(e.to_string()))?
    };
    collector.sample().map_err(|error| match error {
        // `ztop-core`'s client words a 401 this way (client.rs); the
        // live test that reads unpaired keeps the two in step.
        CollectError::Unavailable {
            origin: "agent",
            what,
        } if what.contains("(401)") => ReadError::NeedsPairing,
        CollectError::Io(io) => ReadError::Failed(format!("could not reach the agent: {io}")),
        other => ReadError::Failed(other.to_string()),
    })
}

/// Trades a pairing code for a token over pinned TLS.
pub fn pair(record: &HostRecord, code: &str) -> Result<String, String> {
    if record.pin.is_empty() {
        return Err("pairing needs the agent's TLS fingerprint".into());
    }
    pair_with(&record.target(), &record.pin, code.trim()).map_err(|why| {
        // The agent spaces out pairing attempts after a failure
        // (decision 0061) and answers 429 inside that gap.
        if why.contains("answered 429") {
            "The agent is spacing out pairing attempts. Wait a few seconds and try again."
                .to_string()
        } else {
            why
        }
    })
}

/// What the add-host form holds, checked before anything is saved.
#[derive(Debug, Clone, Default)]
pub struct NewHost {
    pub name: String,
    pub host: String,
    pub port: String,
    pub pin: String,
    pub code: String,
}

/// The agent's default port, shown in the form.
pub const DEFAULT_PORT: u16 = 7100;

impl NewHost {
    /// A record when the form makes sense, else the first problem in
    /// words the form can show.
    pub fn validate(&self) -> Result<HostRecord, String> {
        let host = self.host.trim();
        if host.is_empty() {
            return Err("Enter the host's address.".into());
        }
        if host.chars().any(|c| c.is_whitespace() || c == '/') {
            return Err("The address is a host name or an IP address, nothing more.".into());
        }
        let port = match self.port.trim() {
            "" => DEFAULT_PORT,
            text => match text.parse::<u16>() {
                Ok(port) if port > 0 => port,
                _ => return Err("The port is a number from 1 to 65535.".into()),
            },
        };
        let pin = self.pin.trim();
        if pin.is_empty() {
            if !is_loopback(host) {
                return Err(
                    "A remote host needs its TLS fingerprint (the agent prints it as SHA256:…)."
                        .into(),
                );
            }
            if !self.code.trim().is_empty() {
                return Err("Pairing needs the agent's TLS fingerprint.".into());
            }
        } else if parse_pin(pin).is_none() {
            return Err("The fingerprint looks like SHA256: followed by 64 hex digits.".into());
        }
        let name = match self.name.trim() {
            "" => host,
            name => name,
        };
        Ok(HostRecord::new(
            name,
            host.trim_matches(['[', ']']),
            port,
            pin,
        ))
    }
}

/// Plain HTTP is only for this machine; the agent refuses to serve it
/// anywhere else (decision 0044).
fn is_loopback(host: &str) -> bool {
    let bare = host.trim_matches(['[', ']']);
    bare == "localhost"
        || bare
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PIN: &str = "SHA256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef";

    fn form(host: &str, port: &str, pin: &str, code: &str) -> NewHost {
        NewHost {
            name: String::new(),
            host: host.into(),
            port: port.into(),
            pin: pin.into(),
            code: code.into(),
        }
    }

    #[test]
    fn a_good_form_becomes_a_record_named_after_its_host() {
        let record = form(" host-a.example.com ", "", PIN, "ABCD2345")
            .validate()
            .unwrap();
        assert_eq!(record.name, "host-a.example.com");
        assert_eq!(record.port, DEFAULT_PORT);
        assert_eq!(record.pin, PIN);
        let local = form("127.0.0.1", "7200", "", "").validate().unwrap();
        assert_eq!(local.port, 7200);
        let v6 = form("[::1]", "", "", "").validate().unwrap();
        assert_eq!(v6.host, "::1");
        assert_eq!(v6.target(), "[::1]:7100");
    }

    #[test]
    fn a_bad_form_says_what_is_wrong() {
        assert!(
            form("", "", PIN, "")
                .validate()
                .unwrap_err()
                .contains("address")
        );
        assert!(
            form("host a", "", PIN, "")
                .validate()
                .unwrap_err()
                .contains("address")
        );
        assert!(
            form("host-a", "0", PIN, "")
                .validate()
                .unwrap_err()
                .contains("port")
        );
        assert!(
            form("host-a", "70000", PIN, "")
                .validate()
                .unwrap_err()
                .contains("port")
        );
        assert!(
            form("host-a", "", "", "")
                .validate()
                .unwrap_err()
                .contains("fingerprint")
        );
        assert!(
            form("host-a", "", "SHA256:00", "")
                .validate()
                .unwrap_err()
                .contains("64 hex")
        );
        assert!(
            form("localhost", "", "", "ABCD")
                .validate()
                .unwrap_err()
                .contains("fingerprint")
        );
    }

    #[test]
    fn a_tls_host_without_a_token_needs_pairing_without_dialing() {
        // Port 9 on loopback: nothing should even be tried.
        let record = HostRecord::new("host-a", "127.0.0.1", 9, PIN);
        assert_eq!(read(&record, None).unwrap_err(), ReadError::NeedsPairing);
        let plain = HostRecord::new("host-a", "127.0.0.1", 9, "");
        assert!(matches!(read(&plain, None), Err(ReadError::Failed(_))));
    }
}
