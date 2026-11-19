// SPDX-License-Identifier: MIT OR Apache-2.0
//! TLS for the agent: a self-signed certificate per agent, created on
//! first `--tls` start and kept next to the token store, so a client
//! that pinned the certificate stays pinned across restarts. The
//! fingerprint is the SHA-256 of the certificate's DER, printed by the
//! agent and checked by the client (decision 0003: nothing about TLS
//! is written by hand).

use std::fs;
use std::io;
use std::path::Path;
use std::sync::Arc;

use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use sha2::Digest;

/// How long a freshly generated agent certificate lives. Self-signed
/// and pinned by hash, so it is renewed only by the person running the
/// agent; a short life would break every client's pin.
const VALIDITY_DAYS: i64 = 365 * 10;

/// The ready-to-serve TLS configuration plus the pin clients use.
pub struct TlsMaterial {
    pub config: rustls::ServerConfig,
    /// `SHA256:<hex of the leaf certificate's DER>`.
    pub fingerprint: String,
}

/// Load the agent's certificate and key from `dir`, creating a fresh
/// self-signed pair when they are missing. The directory is 0700 and the
/// key is 0600 from the moment it exists.
pub fn load_or_create(dir: &Path) -> io::Result<TlsMaterial> {
    ztop_core::paths::create_private_dir(dir)?;
    let cert_path = dir.join("agent-cert.pem");
    let key_path = dir.join("agent-key.pem");
    if !cert_path.exists() || !key_path.exists() {
        generate(&cert_path, &key_path)?;
    }
    let cert_pem = fs::read(&cert_path)?;
    let key_pem = fs::read(&key_path)?;
    let certs: Vec<CertificateDer<'static>> = CertificateDer::pem_slice_iter(&cert_pem)
        .collect::<Result<_, _>>()
        .map_err(|error| io::Error::other(format!("bad certificate file: {error}")))?;
    let key: PrivateKeyDer<'static> = PrivateKeyDer::from_pem_slice(&key_pem)
        .map_err(|error| io::Error::other(format!("bad key file: {error}")))?;

    let leaf = certs
        .first()
        .ok_or_else(|| io::Error::other("the certificate file is empty"))?;
    let fingerprint = format!("SHA256:{}", hex(&sha2::Sha256::digest(leaf.as_ref())));

    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .map_err(|error| io::Error::other(format!("TLS versions: {error}")))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|error| io::Error::other(format!("TLS material: {error}")))?;
    Ok(TlsMaterial {
        config,
        fingerprint,
    })
}

/// Generate a self-signed certificate for loopback names and write it
/// with its key. The key file is 0600 on unix.
fn generate(cert_path: &Path, key_path: &Path) -> io::Result<()> {
    let key = rcgen::KeyPair::generate()
        .map_err(|error| io::Error::other(format!("key generation: {error}")))?;
    let mut params =
        rcgen::CertificateParams::new(vec!["localhost".to_string(), "127.0.0.1".to_string()])
            .map_err(|error| io::Error::other(format!("certificate parameters: {error}")))?;
    params
        .distinguished_name
        .push(rcgen::DnType::CommonName, "ztop-agent");
    let now = time::OffsetDateTime::now_utc();
    params.not_before = now - time::Duration::days(1);
    params.not_after = now + time::Duration::days(VALIDITY_DAYS);
    let cert = params
        .self_signed(&key)
        .map_err(|error| io::Error::other(format!("self-signing: {error}")))?;
    // Both through a 0600 temporary file and a rename: the key was
    // written with the default umask and only then tightened, which
    // left a moment where other users could read it (audit M6).
    crate::auth::write_atomically(key_path, key.serialize_pem().as_bytes())?;
    crate::auth::write_atomically(cert_path, cert.pem().as_bytes())?;
    Ok(())
}

/// Lowercase hex, no separators.
fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

/// Serve the agent's router over TLS on an already-bound listener:
/// the seam the integration tests use to get an ephemeral port.
pub async fn serve_on_tls(
    state: Arc<crate::AgentState>,
    listener: tokio::net::TcpListener,
    config: rustls::ServerConfig,
) -> io::Result<()> {
    crate::serving::serve(listener, Some(config), crate::router(state)).await
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// Regression (audit M6): the key was written with the default
    /// umask, then changed to 0600, and the directory was 0755.
    #[test]
    fn the_key_and_its_directory_are_private_from_the_start() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("ztop-tls-perm-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let dir = base.join("state");
        let first = load_or_create(&dir).expect("creates");
        let mode = |path: &Path| fs::metadata(path).expect("meta").permissions().mode() & 0o777;
        assert_eq!(mode(&dir), 0o700);
        assert_eq!(mode(&dir.join("agent-key.pem")), 0o600);
        // A second start loads the same pair.
        let again = load_or_create(&dir).expect("loads");
        assert_eq!(first.fingerprint, again.fingerprint);
        let _ = fs::remove_dir_all(&base);
    }
}
