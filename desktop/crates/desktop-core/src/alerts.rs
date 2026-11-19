// SPDX-License-Identifier: MIT OR Apache-2.0
//! Alert rules and the engine that fires and clears them, with the same
//! rules as the iPad and Mac app: a condition must hold for the rule's
//! duration before it fires, fires once, and clears when it stops
//! holding. Rules live in `alerts.json` next to `hosts.json`; the
//! engine's timers are in memory only, so they start over at launch.

use std::collections::HashMap;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use ztop_core::model::Sample;

/// What a rule watches. The threshold is a percent for CPU, memory and
/// disk and an absolute value for load; `Unreachable` has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AlertKind {
    Cpu,
    Memory,
    Load,
    Disk,
    Unreachable,
}

impl AlertKind {
    pub const ALL: [AlertKind; 5] = [
        Self::Cpu,
        Self::Memory,
        Self::Load,
        Self::Disk,
        Self::Unreachable,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Memory => "Memory",
            Self::Load => "Load",
            Self::Disk => "Disk",
            Self::Unreachable => "Unreachable",
        }
    }

    pub fn uses_threshold(self) -> bool {
        self != Self::Unreachable
    }
}

/// One saved rule. `host` is a record id, or `None` for every host.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AlertRule {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    pub kind: AlertKind,
    #[serde(default)]
    pub threshold: f64,
    /// Seconds the condition must hold before the rule fires.
    pub duration_secs: u64,
}

impl AlertRule {
    pub fn new(
        name: &str,
        host: Option<String>,
        kind: AlertKind,
        threshold: f64,
        duration_secs: u64,
    ) -> Self {
        let mut bytes = [0u8; 8];
        getrandom::fill(&mut bytes).expect("the OS random source");
        Self {
            id: bytes.iter().map(|b| format!("{b:02x}")).collect(),
            name: name.to_string(),
            host,
            kind,
            threshold,
            duration_secs,
        }
    }

    /// The condition in words: "CPU above 90% for 5m".
    pub fn describe(&self) -> String {
        let target = if self.host.is_none() {
            "any host: "
        } else {
            ""
        };
        let duration = duration_text(self.duration_secs);
        match self.kind {
            AlertKind::Cpu => format!("{target}CPU above {:.0}% for {duration}", self.threshold),
            AlertKind::Memory => {
                format!("{target}memory above {:.0}% for {duration}", self.threshold)
            }
            AlertKind::Load => format!("{target}load above {:.1} for {duration}", self.threshold),
            AlertKind::Disk => format!("{target}disk above {:.0}% for {duration}", self.threshold),
            AlertKind::Unreachable => format!("{target}unreachable for {duration}"),
        }
    }

    fn watches(&self, host: &str) -> bool {
        self.host.as_deref().is_none_or(|h| h == host)
    }

    fn is_violating(&self, sample: Option<&Sample>, unreachable: bool) -> bool {
        if self.kind == AlertKind::Unreachable {
            return unreachable;
        }
        let Some(sample) = sample else {
            return false;
        };
        match self.kind {
            AlertKind::Cpu => {
                let busy = &sample.cpu.busy_percent;
                !busy.is_empty() && busy.iter().sum::<f64>() / busy.len() as f64 > self.threshold
            }
            AlertKind::Memory => {
                let m = &sample.memory;
                m.total.available
                    && m.used.available
                    && m.total.value > 0.0
                    && m.used.value / m.total.value * 100.0 > self.threshold
            }
            AlertKind::Load => sample.load_avg[0] > self.threshold,
            AlertKind::Disk => sample
                .disks
                .iter()
                .filter(|d| d.total_bytes > 0)
                .any(|d| d.usage_pct > self.threshold),
            AlertKind::Unreachable => unreachable,
        }
    }
}

/// "45s", "5m".
pub fn duration_text(seconds: u64) -> String {
    if seconds >= 60 && seconds % 60 == 0 {
        format!("{}m", seconds / 60)
    } else {
        format!("{seconds}s")
    }
}

/// What a tick decided for one rule and host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AlertEvent {
    Fired {
        rule: String,
        host: String,
        name: String,
    },
    Cleared {
        rule: String,
        host: String,
        name: String,
    },
}

#[derive(Debug, Default, Clone, Copy)]
struct Track {
    since: Option<Instant>,
    active: bool,
}

/// Fires and clears rules as hosts are read; one track per rule and
/// host.
#[derive(Debug, Default)]
pub struct AlertEngine {
    tracks: HashMap<(String, String), Track>,
}

impl AlertEngine {
    /// Evaluates every rule that watches `host` against its newest
    /// state. A missing sample reads as no metric violating; an
    /// `Unreachable` rule violates only while `unreachable` holds.
    pub fn evaluate(
        &mut self,
        rules: &[AlertRule],
        host: &str,
        sample: Option<&Sample>,
        unreachable: bool,
        now: Instant,
    ) -> Vec<AlertEvent> {
        let mut events = Vec::new();
        for rule in rules.iter().filter(|r| r.watches(host)) {
            let key = (rule.id.clone(), host.to_string());
            let track = self.tracks.entry(key).or_default();
            if rule.is_violating(sample, unreachable) {
                let since = *track.since.get_or_insert(now);
                if !track.active
                    && now.duration_since(since) >= Duration::from_secs(rule.duration_secs)
                {
                    track.active = true;
                    events.push(AlertEvent::Fired {
                        rule: rule.id.clone(),
                        host: host.to_string(),
                        name: rule.name.clone(),
                    });
                }
            } else if track.since.is_some() {
                if track.active {
                    events.push(AlertEvent::Cleared {
                        rule: rule.id.clone(),
                        host: host.to_string(),
                        name: rule.name.clone(),
                    });
                }
                *track = Track::default();
            }
        }
        // Rules deleted or narrowed to another host leave no tracks.
        self.tracks.retain(|(rule, h), _| {
            h != host || rules.iter().any(|r| &r.id == rule && r.watches(host))
        });
        events
    }

    /// The hosts a rule is firing for now.
    pub fn firing(&self, rule: &str) -> Vec<String> {
        let mut hosts: Vec<String> = self
            .tracks
            .iter()
            .filter(|((r, _), t)| r == rule && t.active)
            .map(|((_, h), _)| h.clone())
            .collect();
        hosts.sort();
        hosts
    }
}

/// The saved rules and the file they live in.
#[derive(Debug)]
pub struct AlertBook {
    file: PathBuf,
    pub rules: Vec<AlertRule>,
}

impl AlertBook {
    /// Loads `dir/alerts.json`; missing is no rules, unreadable is an
    /// error rather than a silent wipe on the next save.
    pub fn load(dir: &Path) -> std::io::Result<Self> {
        let file = dir.join("alerts.json");
        let rules = match std::fs::read_to_string(&file) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => return Err(e),
        };
        Ok(Self { file, rules })
    }

    /// Writes the rules atomically, mode 0600.
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
            serde_json::to_writer_pretty(&mut out, &self.rules)?;
            out.write_all(b"\n")?;
            out.sync_all()?;
        }
        std::fs::rename(&tmp, &self.file)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ztop_core::model::{Metric, Unit};

    fn busy(percent: f64) -> Sample {
        let mut sample = crate::blank_sample();
        sample.cpu.busy_percent = vec![percent, percent];
        sample
    }

    fn rule(kind: AlertKind, threshold: f64, seconds: u64) -> AlertRule {
        AlertRule::new("hot", None, kind, threshold, seconds)
    }

    #[test]
    fn a_rule_fires_after_its_duration_once_and_clears() {
        let rules = [rule(AlertKind::Cpu, 80.0, 30)];
        let mut engine = AlertEngine::default();
        let t0 = Instant::now();
        let at = |s| t0 + Duration::from_secs(s);
        let hot = busy(95.0);
        assert!(
            engine
                .evaluate(&rules, "a", Some(&hot), false, at(0))
                .is_empty()
        );
        assert!(
            engine
                .evaluate(&rules, "a", Some(&hot), false, at(29))
                .is_empty()
        );
        let fired = engine.evaluate(&rules, "a", Some(&hot), false, at(30));
        assert!(matches!(&fired[..], [AlertEvent::Fired { host, .. }] if host == "a"));
        assert_eq!(engine.firing(&rules[0].id), ["a"]);
        assert!(
            engine
                .evaluate(&rules, "a", Some(&hot), false, at(60))
                .is_empty(),
            "fires once"
        );
        let cleared = engine.evaluate(&rules, "a", Some(&busy(10.0)), false, at(61));
        assert!(matches!(&cleared[..], [AlertEvent::Cleared { .. }]));
        assert!(engine.firing(&rules[0].id).is_empty());
    }

    #[test]
    fn a_dip_restarts_the_timer_without_an_event() {
        let rules = [rule(AlertKind::Cpu, 80.0, 30)];
        let mut engine = AlertEngine::default();
        let t0 = Instant::now();
        let at = |s| t0 + Duration::from_secs(s);
        engine.evaluate(&rules, "a", Some(&busy(95.0)), false, at(0));
        assert!(
            engine
                .evaluate(&rules, "a", Some(&busy(10.0)), false, at(20))
                .is_empty()
        );
        engine.evaluate(&rules, "a", Some(&busy(95.0)), false, at(25));
        assert!(
            engine
                .evaluate(&rules, "a", Some(&busy(95.0)), false, at(50))
                .is_empty()
        );
        assert_eq!(
            engine
                .evaluate(&rules, "a", Some(&busy(95.0)), false, at(55))
                .len(),
            1
        );
    }

    #[test]
    fn host_rules_watch_one_host_and_unreachable_needs_no_sample() {
        let mut only_b = rule(AlertKind::Unreachable, 0.0, 0);
        only_b.host = Some("b".into());
        let rules = [only_b];
        let mut engine = AlertEngine::default();
        let now = Instant::now();
        assert!(engine.evaluate(&rules, "a", None, true, now).is_empty());
        assert_eq!(engine.evaluate(&rules, "b", None, true, now).len(), 1);
        // Without a sample, metric rules never violate.
        let cpu = [rule(AlertKind::Cpu, 0.0, 0)];
        assert!(engine.evaluate(&cpu, "a", None, false, now).is_empty());
    }

    #[test]
    fn memory_load_and_disk_read_their_figures() {
        let mut sample = crate::blank_sample();
        sample.memory.total = Metric::available(100.0, Unit::Bytes);
        sample.memory.used = Metric::available(91.0, Unit::Bytes);
        sample.load_avg = [4.5, 1.0, 1.0];
        let now = Instant::now();
        let fires = |r: AlertRule| {
            !AlertEngine::default()
                .evaluate(&[r], "a", Some(&sample), false, now)
                .is_empty()
        };
        assert!(fires(rule(AlertKind::Memory, 90.0, 0)));
        assert!(!fires(rule(AlertKind::Memory, 95.0, 0)));
        assert!(fires(rule(AlertKind::Load, 4.0, 0)));
        assert!(
            !fires(rule(AlertKind::Disk, 0.0, 0)),
            "no disks reported, no disk alert"
        );
    }

    #[test]
    fn deleted_rules_leave_no_tracks() {
        let rules = vec![rule(AlertKind::Cpu, 0.0, 0)];
        let mut engine = AlertEngine::default();
        engine.evaluate(&rules, "a", Some(&busy(50.0)), false, Instant::now());
        assert_eq!(engine.firing(&rules[0].id), ["a"]);
        engine.evaluate(&[], "a", Some(&busy(50.0)), false, Instant::now());
        assert!(engine.firing(&rules[0].id).is_empty());
    }

    #[test]
    fn rules_describe_themselves_and_round_trip_the_file() {
        let mut r = rule(AlertKind::Cpu, 90.0, 300);
        assert_eq!(r.describe(), "any host: CPU above 90% for 5m");
        r.host = Some("x".into());
        r.kind = AlertKind::Unreachable;
        r.duration_secs = 45;
        assert_eq!(r.describe(), "unreachable for 45s");

        let dir = std::env::temp_dir().join(format!("ztop-alerts-{}", r.id));
        let mut book = AlertBook::load(&dir).unwrap();
        assert!(book.rules.is_empty());
        book.rules.push(r.clone());
        book.save().unwrap();
        assert_eq!(AlertBook::load(&dir).unwrap().rules, vec![r]);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
