// SPDX-License-Identifier: MIT OR Apache-2.0
//! The detail page's process table: search, sort, and the signals a
//! row's menu offers, named and numbered by the target agent.

use std::cmp::Ordering;

use ztop_core::model::Process;

/// What the table sorts by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortKey {
    Cpu,
    Memory,
    Pid,
    Name,
    User,
}

impl SortKey {
    pub const ALL: [SortKey; 5] = [Self::Cpu, Self::Memory, Self::Pid, Self::Name, Self::User];

    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Memory => "Memory",
            Self::Pid => "PID",
            Self::Name => "Name",
            Self::User => "User",
        }
    }

    /// Busy and big first; names, users and pids from the top.
    pub fn descending_by_default(self) -> bool {
        matches!(self, Self::Cpu | Self::Memory)
    }
}

/// The table's search and order.
#[derive(Debug, Clone, PartialEq)]
pub struct Query {
    pub search: String,
    pub sort: SortKey,
    pub descending: bool,
}

impl Default for Query {
    fn default() -> Self {
        Self {
            search: String::new(),
            sort: SortKey::Cpu,
            descending: true,
        }
    }
}

impl Query {
    /// A search matches the name or user without regard to case, or
    /// the start of the pid.
    pub fn matches(&self, process: &Process) -> bool {
        let needle = self.search.trim().to_lowercase();
        needle.is_empty()
            || process.name.to_lowercase().contains(&needle)
            || process.user.to_lowercase().contains(&needle)
            || process.pid.to_string().starts_with(&needle)
    }

    /// The matching processes in order. Ties fall back to the pid,
    /// ascending in both directions, so equal rows never swap places
    /// between polls.
    pub fn table<'a>(&self, processes: &'a [Process]) -> Vec<&'a Process> {
        let mut rows: Vec<&Process> = processes.iter().filter(|p| self.matches(p)).collect();
        rows.sort_by(|a, b| {
            let primary = match self.sort {
                SortKey::Cpu => a.cpu_percent.total_cmp(&b.cpu_percent),
                SortKey::Memory => a.mem_percent.total_cmp(&b.mem_percent),
                SortKey::Pid => a.pid.cmp(&b.pid),
                SortKey::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortKey::User => a.user.to_lowercase().cmp(&b.user.to_lowercase()),
            };
            let primary = if self.descending {
                primary.reverse()
            } else {
                primary
            };
            primary.then(a.pid.cmp(&b.pid))
        });
        rows
    }
}

/// One entry of the target's signal catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signal {
    pub name: String,
    pub number: i32,
}

impl Signal {
    /// KILL cannot be caught, so the process gets no chance to clean
    /// up: the menu asks first.
    pub fn needs_confirmation(&self) -> bool {
        self.name.eq_ignore_ascii_case("KILL") || self.name.eq_ignore_ascii_case("SIGKILL")
    }
}

/// The catalog in menu order: the everyday signals first (TERM, then
/// STOP and CONT, then KILL last of those), the rest by number.
pub fn menu_order(catalog: Vec<(String, i32)>) -> Vec<Signal> {
    const FIRST: [&str; 5] = ["TERM", "HUP", "INT", "STOP", "CONT"];
    let rank = |name: &str| {
        FIRST
            .iter()
            .position(|f| f.eq_ignore_ascii_case(name.trim_start_matches("SIG")))
            .unwrap_or(FIRST.len())
    };
    let mut signals: Vec<Signal> = catalog
        .into_iter()
        .filter(|(_, number)| *number > 0)
        .map(|(name, number)| Signal { name, number })
        .collect();
    signals.sort_by(|a, b| match (rank(&a.name), rank(&b.name)) {
        (x, y) if x != y => x.cmp(&y),
        _ => {
            // KILL closes the everyday group; the rest go by number.
            let kill = |s: &Signal| s.needs_confirmation();
            match (kill(a), kill(b)) {
                (true, false) => Ordering::Less,
                (false, true) => Ordering::Greater,
                _ => a.number.cmp(&b.number),
            }
        }
    });
    signals
}

/// How a control result reads in a toast.
pub fn outcome_text(signal: &str, name: &str, pid: u32, result: &Result<(), String>) -> String {
    match result {
        Ok(()) => format!("Sent {signal} to {name} ({pid})."),
        Err(why) => format!("{signal} to {name} ({pid}) failed: {why}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn process(pid: u32, name: &str, user: &str, cpu: f64, mem: f64) -> Process {
        serde_json::from_value(serde_json::json!({
            "pid": pid, "parent": null, "user": user, "state": "S", "name": name,
            "cpu_percent": cpu, "mem_percent": mem, "virtual_bytes": 0,
            "resident_bytes": 0, "threads": 1,
            "io": { "read_bytes": null, "write_bytes": null, "read_rate": null, "write_rate": null },
        }))
        .expect("a process")
    }

    fn rows() -> Vec<Process> {
        vec![
            process(30, "bash", "alice", 1.0, 0.5),
            process(4, "Xorg", "root", 12.0, 2.0),
            process(12, "cargo", "alice", 12.0, 8.0),
            process(301, "sshd", "root", 0.0, 0.1),
        ]
    }

    fn pids(rows: &[&Process]) -> Vec<u32> {
        rows.iter().map(|p| p.pid).collect()
    }

    #[test]
    fn cpu_sorts_busiest_first_with_ties_by_pid_both_ways() {
        let processes = rows();
        let query = Query::default();
        assert_eq!(pids(&query.table(&processes)), [4, 12, 30, 301]);
        let ascending = Query {
            descending: false,
            ..Query::default()
        };
        assert_eq!(pids(&ascending.table(&processes)), [301, 30, 4, 12]);
    }

    #[test]
    fn other_keys_sort_as_named() {
        let processes = rows();
        let by = |sort, descending| Query {
            search: String::new(),
            sort,
            descending,
        };
        assert_eq!(
            pids(&by(SortKey::Memory, true).table(&processes)),
            [12, 4, 30, 301]
        );
        assert_eq!(
            pids(&by(SortKey::Pid, false).table(&processes)),
            [4, 12, 30, 301]
        );
        assert_eq!(
            pids(&by(SortKey::Name, false).table(&processes)),
            [30, 12, 301, 4]
        );
        assert_eq!(
            pids(&by(SortKey::User, false).table(&processes)),
            [12, 30, 4, 301]
        );
    }

    #[test]
    fn search_matches_name_user_or_pid_prefix() {
        let processes = rows();
        let search = |text: &str| Query {
            search: text.into(),
            ..Query::default()
        };
        assert_eq!(pids(&search("XORG").table(&processes)), [4]);
        assert_eq!(pids(&search("alice").table(&processes)), [12, 30]);
        assert_eq!(pids(&search("30").table(&processes)), [30, 301]);
        assert_eq!(pids(&search("  ").table(&processes)).len(), 4);
        assert!(search("nothing").table(&processes).is_empty());
    }

    #[test]
    fn the_menu_puts_everyday_signals_first_and_kill_after_them() {
        let catalog = [
            ("HUP", 1),
            ("INT", 2),
            ("KILL", 9),
            ("USR1", 10),
            ("TERM", 15),
            ("CONT", 18),
            ("STOP", 19),
            ("PROBE", 0),
        ]
        .map(|(n, v)| (n.to_string(), v))
        .to_vec();
        let names: Vec<String> = menu_order(catalog).into_iter().map(|s| s.name).collect();
        assert_eq!(
            names,
            ["TERM", "HUP", "INT", "STOP", "CONT", "KILL", "USR1"]
        );
    }

    #[test]
    fn only_kill_asks_first() {
        let signal = |name: &str| Signal {
            name: name.into(),
            number: 1,
        };
        assert!(signal("KILL").needs_confirmation());
        assert!(signal("SIGKILL").needs_confirmation());
        assert!(!signal("TERM").needs_confirmation());
    }

    #[test]
    fn outcomes_read_as_sentences() {
        assert_eq!(
            outcome_text("TERM", "bash", 30, &Ok(())),
            "Sent TERM to bash (30)."
        );
        assert_eq!(
            outcome_text("STOP", "sshd", 301, &Err("permission denied".into())),
            "STOP to sshd (301) failed: permission denied"
        );
    }
}
