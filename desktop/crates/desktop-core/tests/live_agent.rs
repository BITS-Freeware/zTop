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
    start_agent_with(bin, tls, &[], false)
}

/// [`start_agent`] with extra agent flags, such as `--allow-control`.
fn start_agent_with(bin: &PathBuf, tls: bool, extra: &[&str], advertise: bool) -> Agent {
    let home = temp_home("agent");
    let port = random_port();
    let mut args = vec!["agent".to_string(), "--port".into(), port.to_string()];
    if tls {
        args.extend(["--tls", "--pair"].map(String::from));
        if !advertise {
            args.push("--no-advertise".into());
        }
    }
    args.extend(extra.iter().map(|a| a.to_string()));
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

#[test]
fn the_detail_page_history_comes_from_the_agent_in_time_order() {
    let Some(bin) = agent_binary() else { return };
    let agent = start_agent(&bin, true);
    let record = HostRecord::new("host-a", "127.0.0.1", agent.port, &agent.pin);
    let token = agent::pair(&record, &agent.code).expect("pairing with the printed code");
    read_soon(&record, Some(token.clone())).expect("a paired read");
    // The agent samples once a second; give it a few.
    std::thread::sleep(Duration::from_secs(3));

    let samples = agent::history(&record, Some(token.clone()), 60).expect("the agent's history");
    assert!(samples.len() >= 2, "only {} samples", samples.len());
    assert!(
        samples
            .windows(2)
            .all(|w| w[0].timestamp_ms < w[1].timestamp_ms),
        "the history is oldest first"
    );
    let mut history = ztop_desktop_core::History::default();
    history.extend(samples);
    assert!(!history.cpu().is_empty());
    assert_eq!(history.load().len(), 3);

    // Without the token the history is refused like a snapshot.
    assert_eq!(
        agent::history(&record, None, 60).unwrap_err(),
        ReadError::NeedsPairing
    );
}

/// The state letter `/proc/<pid>/stat` reports: `T` stopped, `S`
/// sleeping, `Z` exited but not yet reaped.
fn proc_state(pid: u32) -> Option<char> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    stat.rsplit_once(") ")?.1.chars().next()
}

fn wait_for_state(pid: u32, wanted: char) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if proc_state(pid) == Some(wanted) {
            return true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn stop_cont_and_term_reach_a_process_by_the_agents_numbers() {
    let Some(bin) = agent_binary() else { return };

    // Without --allow-control the agent refuses in its own words.
    let plain = start_agent(&bin, true);
    let plain_record = HostRecord::new("host-a", "127.0.0.1", plain.port, &plain.pin);
    let plain_token = agent::pair(&plain_record, &plain.code).expect("pairing");
    let refused = agent::send_signal(&plain_record, Some(plain_token), &[1], 0);
    let why = refused[0].1.clone().unwrap_err();
    assert!(why.contains("allow"), "unexpected refusal: {why}");

    let agent = start_agent_with(&bin, true, &["--allow-control"], false);
    let record = HostRecord::new("host-b", "127.0.0.1", agent.port, &agent.pin);
    let token = agent::pair(&record, &agent.code).expect("pairing");
    read_soon(&record, Some(token.clone())).expect("a paired read");

    let catalog = agent::signal_catalog(&record, Some(token.clone())).expect("the catalog");
    let number = |name: &str| {
        catalog
            .iter()
            .find(|s| s.name == name)
            .unwrap_or_else(|| panic!("{name} is not in the catalog"))
            .number
    };
    assert_eq!(catalog[0].name, "TERM", "the menu starts with TERM");

    let mut child = Command::new("sleep")
        .arg("300")
        .spawn()
        .expect("spawn sleep");
    let pid = child.id();
    let send = |name: &str| {
        let results = agent::send_signal(&record, Some(token.clone()), &[pid], number(name));
        assert_eq!(results.len(), 1);
        results[0].1.clone()
    };

    send("STOP").expect("STOP");
    assert!(wait_for_state(pid, 'T'), "STOP did not stop the process");
    send("CONT").expect("CONT");
    assert!(wait_for_state(pid, 'S'), "CONT did not resume the process");
    send("TERM").expect("TERM");
    let status = child.wait().expect("the child ends");
    use std::os::unix::process::ExitStatusExt;
    assert_eq!(status.signal(), Some(15), "TERM ended the process");
}

#[test]
fn three_agents_read_back_live_through_one_hub() {
    let Some(bin) = agent_binary() else { return };
    let agents: Vec<Agent> = (0..3).map(|_| start_agent(&bin, false)).collect();
    let names = ["host-a", "host-b", "host-c"];

    let home = temp_home("hub");
    let list: Vec<String> = agents
        .iter()
        .zip(names)
        .map(|(a, n)| format!(r#"{{"name":"{n}","target":"127.0.0.1:{}"}}"#, a.port))
        .collect();
    let file = home.join("agents.json");
    std::fs::write(&file, format!(r#"{{"agents":[{}]}}"#, list.join(","))).unwrap();
    let port = random_port();
    let mut child = Command::new(&bin)
        .args(["hub", "--tls", "--pair", "--port", &port.to_string()])
        .arg("--agents")
        .arg(&file)
        .env_clear()
        .env("HOME", &home)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn ztop hub");
    let stdout = child.stdout.take().unwrap();
    let hub = Agent {
        child,
        home,
        port,
        pin: String::new(),
        code: String::new(),
    };
    let (mut pin, mut code) = (String::new(), String::new());
    let mut lines = BufReader::new(stdout).lines().map_while(Result::ok);
    while pin.is_empty() || code.is_empty() {
        let Some(line) = lines.next() else { break };
        if let Some(rest) = line.strip_prefix("Pairing code: ") {
            code = rest.split_whitespace().next().unwrap_or_default().into();
        }
        if let Some(rest) = line.strip_prefix("TLS fingerprint: ") {
            pin = rest.trim().into();
        }
    }
    assert!(
        !pin.is_empty() && !code.is_empty(),
        "the hub printed no pairing lines"
    );

    let record = HostRecord::new("hub-1", "127.0.0.1", hub.port, &pin).as_hub();
    let token = agent::pair(&record, &code).expect("pairing with the hub");
    let deadline = Instant::now() + Duration::from_secs(20);
    let hosts = loop {
        let hosts = agent::hub_hosts(&record, Some(token.clone())).unwrap_or_default();
        if hosts.len() == 3 && hosts.iter().all(|h| h.state == "live") {
            break hosts;
        }
        assert!(
            Instant::now() < deadline,
            "the hub never saw all three live: {hosts:?}"
        );
        std::thread::sleep(Duration::from_millis(300));
    };
    let mut seen: Vec<&str> = hosts.iter().map(|h| h.name.as_str()).collect();
    seen.sort();
    assert_eq!(seen, names);

    for name in names {
        let machine = record.machine(name);
        let sample = agent::read(&machine, Some(token.clone())).expect("a read through the hub");
        assert!(!sample.cpu.cores.is_empty(), "{name} read empty");
        let history = agent::history(&machine, Some(token.clone()), 60).expect("history");
        assert!(!history.is_empty(), "{name} has no history through the hub");
    }
    assert_eq!(
        agent::hub_hosts(&record, None).unwrap_err(),
        ReadError::NeedsPairing
    );
    drop(hub);
}

/// Needs working multicast, which a CI container may not have: run it
/// with `ZTOP_TEST_MULTICAST=1` where Bonjour works (check.sh does).
#[test]
fn an_advertised_agent_is_found_with_its_port_on_loopback() {
    if std::env::var("ZTOP_TEST_MULTICAST").as_deref() != Ok("1") {
        eprintln!("skipped: set ZTOP_TEST_MULTICAST=1 where multicast works");
        return;
    }
    let Some(bin) = agent_binary() else { return };
    let agent = start_agent_with(&bin, true, &[], true);
    let mut browser = ztop_desktop_core::discovery::Browser::start().expect("a browser");
    let deadline = Instant::now() + Duration::from_secs(15);
    let found = loop {
        browser.poll();
        if let Some(found) = browser.found().into_iter().find(|f| f.port == agent.port) {
            break found;
        }
        assert!(
            Instant::now() < deadline,
            "the agent on port {} was not found",
            agent.port
        );
        std::thread::sleep(Duration::from_millis(200));
    };
    assert_eq!(
        found.host, "127.0.0.1",
        "this machine's agent is reached on loopback"
    );
    assert!(found.compatible());
    assert!(!found.name.is_empty() && !found.name.starts_with("ztop on "));

    // The address found is one the form can save and pair with.
    let record = HostRecord::new(&found.name, &found.host, found.port, &agent.pin);
    agent::pair(&record, &agent.code).expect("pairing with the found agent");
}
