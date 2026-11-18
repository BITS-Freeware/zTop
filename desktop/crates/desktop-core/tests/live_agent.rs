// SPDX-License-Identifier: MIT OR Apache-2.0
//! The app's agent calls against a real `ztop agent`, spawned from the
//! workspace build on loopback with its own HOME. `ZTOP_BIN` names the
//! binary; otherwise `target/debug/ztop` at the repository root. Where neither exists
//! the tests say so and pass, unless `ZTOP_REQUIRE_LIVE=1` (CI) makes
//! a missing binary a failure.

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use ztop_desktop_core::agent::{self, ReadError};
use ztop_desktop_core::{FileTokenStore, HostLive, HostRecord, HostState, TokenStore};

fn agent_binary() -> Option<PathBuf> {
    if let Some(bin) = std::env::var_os("ZTOP_BIN").filter(|b| !b.is_empty()) {
        return Some(bin.into());
    }
    let built = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../../target/debug/ztop");
    if built.exists() {
        return Some(built);
    }
    assert!(
        std::env::var("ZTOP_REQUIRE_LIVE").as_deref() != Ok("1"),
        "no ztop binary: cargo build -p ztop first"
    );
    eprintln!("skipping: no ztop binary (cargo build -p ztop at the repository root)");
    None
}

/// A running agent; killed and its HOME removed on drop.
struct Agent {
    child: Child,
    home: PathBuf,
    port: u16,
    pin: String,
    code: String,
}

impl Drop for Agent {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn random_port() -> u16 {
    let mut b = [0u8; 2];
    getrandom::fill(&mut b).unwrap();
    20000 + u16::from_le_bytes(b) % 25000
}

fn temp_home(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "ztop-desktop-live-{tag}-{}",
        HostRecord::new("x", "x", 1, "").id
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// Starts `ztop agent` and waits for its startup lines: with `tls`,
/// the pairing code and the fingerprint; plain, the first line.
fn start_agent(bin: &PathBuf, tls: bool) -> Agent {
    let home = temp_home("agent");
    let port = random_port();
    let mut args = vec!["agent".to_string(), "--port".into(), port.to_string()];
    if tls {
        args.extend(["--tls", "--pair", "--no-advertise"].map(String::from));
    }
    let mut child = Command::new(bin)
        .args(&args)
        .env_clear()
        .env("HOME", &home)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ztop agent");
    let stdout = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    let (mut pin, mut code) = (String::new(), String::new());
    let deadline = Instant::now() + Duration::from_secs(30);
    while Instant::now() < deadline && (tls && (pin.is_empty() || code.is_empty())) {
        let Ok(line) = rx.recv_timeout(Duration::from_millis(200)) else {
            continue;
        };
        if let Some(rest) = line.strip_prefix("Pairing code: ") {
            code = rest
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string();
        }
        if let Some(rest) = line.strip_prefix("TLS fingerprint: ") {
            pin = rest.trim().to_string();
        }
    }
    let agent = Agent {
        child,
        home,
        port,
        pin,
        code,
    };
    assert!(
        !tls || (!agent.pin.is_empty() && !agent.code.is_empty()),
        "the agent printed no pairing lines"
    );
    agent
}

/// Reads until the agent answers or ten seconds pass (it may still be
/// binding its port).
fn read_soon(
    record: &HostRecord,
    token: Option<String>,
) -> Result<ztop_core::model::Sample, ReadError> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let result = agent::read(record, token.clone());
        if matches!(result, Err(ReadError::Failed(_))) && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(200));
            continue;
        }
        return result;
    }
}

#[test]
fn pairing_turns_a_refused_read_into_a_live_one() {
    let Some(bin) = agent_binary() else { return };
    let agent = start_agent(&bin, true);
    let record = HostRecord::new("host-a", "127.0.0.1", agent.port, &agent.pin);

    // A token the agent never issued draws a real 401, which must read
    // as "pair", not as a broken host.
    assert_eq!(
        read_soon(&record, Some("not-a-token".into())).unwrap_err(),
        ReadError::NeedsPairing
    );

    let token = agent::pair(&record, &agent.code).expect("pairing with the printed code");
    // The code is single-use, and a wrong one never pairs.
    assert!(
        agent::pair(&record, &agent.code).is_err(),
        "a used code must not pair again"
    );
    assert!(
        agent::pair(&record, "WRONG234").is_err(),
        "a wrong code must not pair"
    );

    let config = temp_home("config");
    let store = FileTokenStore::new(&config);
    store.save(&record.id, &token).unwrap();
    let sample = read_soon(&record, store.load(&record.id)).expect("a paired read");
    assert!(!sample.hostname.is_empty());
    assert!(!sample.cpu.cores.is_empty());

    let mut live = HostLive::default();
    live.apply(Ok(sample), Instant::now());
    assert_eq!(live.state, HostState::Live);
    std::fs::remove_dir_all(config).unwrap();
}

#[test]
fn a_wrong_pin_fails_and_a_stopped_agent_goes_stale() {
    let Some(bin) = agent_binary() else { return };
    let agent = start_agent(&bin, true);
    let good = HostRecord::new("host-b", "127.0.0.1", agent.port, &agent.pin);
    // Wait for the port, then pair, then for the agent's first sample
    // (it answers 503 until its sampler has produced one).
    let _ = read_soon(&good, Some("x".into()));
    let token = agent::pair(&good, &agent.code).expect("pairing");
    let first_read = read_soon(&good, Some(token.clone()));

    // Flip the first hex digit so the pin can never equal the real one.
    let mut wrong_pin = agent.pin.clone();
    let first = wrong_pin.find(':').unwrap() + 1;
    let flipped = if &wrong_pin[first..first + 1] == "0" {
        "1"
    } else {
        "0"
    };
    wrong_pin.replace_range(first..first + 1, flipped);
    let wrong = HostRecord {
        pin: wrong_pin,
        ..good.clone()
    };
    assert!(matches!(
        agent::read(&wrong, Some(token.clone())),
        Err(ReadError::Failed(_))
    ));

    let mut live = HostLive::default();
    let t0 = Instant::now();
    live.apply(first_read, t0);
    assert_eq!(live.state, HostState::Live, "{:?}", live.error);
    drop(agent);
    live.apply(agent::read(&good, Some(token)), t0 + Duration::from_secs(2));
    assert_eq!(live.state, HostState::Stale);
    assert!(live.error.is_some());
}

#[test]
fn a_plain_loopback_agent_reads_without_pairing() {
    let Some(bin) = agent_binary() else { return };
    let agent = start_agent(&bin, false);
    let record = HostRecord::new("host-c", "127.0.0.1", agent.port, "");
    let sample = read_soon(&record, None).expect("a plain loopback read");
    assert!(!sample.cpu.cores.is_empty());
}
