// SPDX-License-Identifier: MIT OR Apache-2.0
//! Decode and unit math shared by the power-related collectors.
//!
//! The macOS sources behind these helpers are fixed by decision 0011:
//! SMC float values are little-endian in memory, IOReport energy counters
//! count in 1e-9 J units, and CPU state residency counts in mach ticks
//! with cumulative per-state values where idle states are named `ELDI`
//! and similar. The math here is pure, so tests run on every platform,
//! while the sources live in the platform collectors.

/// A little-endian 32-bit float, as SMC stores `flt ` values.
pub fn f32_le4(bytes: &[u8]) -> Option<f32> {
    let word: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
    Some(f32::from_le_bytes(word))
}

/// Watts from an IOReport energy counter delta.
///
/// Energy channels count in 1e-9 J units (nanjoules), so the power over
/// the interval is the delta divided by the elapsed time, rescaled. A
/// non-positive elapsed time or a negative delta (counter reset) yields
/// no value rather than a wrong one.
pub fn watts_from_energy_delta(delta: i64, elapsed_secs: f64) -> Option<f64> {
    if elapsed_secs <= 0.0 || delta < 0 {
        return None;
    }
    Some(delta as f64 * 1e-9 / elapsed_secs)
}

/// Busy share of one CPU core from its state residency ticks.
///
/// `states` pairs state names with cumulative residency ticks; the busy
/// share is everything that is not idle. Idle states are named `ELDI` on
/// current Apple silicon and may appear as `IDLE`, `*LDI` or `OFF`
/// elsewhere (decision 0011). Returns `None` when no ticks were
/// observed, because a share cannot be computed from nothing.
pub fn busy_share_from_states(states: &[(&str, i64)]) -> Option<f64> {
    let mut total: i64 = 0;
    let mut idle: i64 = 0;
    for (name, ticks) in states {
        if *ticks < 0 {
            continue;
        }
        total += ticks;
        if *name == "ELDI" || name.ends_with("LDI") || name.contains("IDLE") || name.contains("OFF")
        {
            idle += ticks;
        }
    }
    if total <= 0 {
        return None;
    }
    Some((1.0 - idle as f64 / total as f64).clamp(0.0, 1.0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f32_reads_little_endian_words() {
        let bytes = 28.5f32.to_le_bytes();
        assert_eq!(f32_le4(&bytes), Some(28.5));
        assert_eq!(f32_le4(&bytes[..3]), None);
    }

    #[test]
    fn energy_delta_becomes_watts() {
        // 34,769,592,692 nanjoules over ~4 s is the measured Metal-load
        // GPU power from the probe (decision 0011).
        let watts = watts_from_energy_delta(34_769_592_692, 4.0).unwrap();
        assert!((watts - 8.692_4).abs() < 0.01, "{watts}");
    }

    #[test]
    fn energy_delta_refuses_bad_intervals() {
        assert_eq!(watts_from_energy_delta(1_000, 0.0), None);
        assert_eq!(watts_from_energy_delta(1_000, -1.0), None);
        assert_eq!(watts_from_energy_delta(-5, 1.0), None);
    }

    #[test]
    fn busy_share_keys_on_the_idle_state_names() {
        let states = vec![("ELDI", 750), ("0P6V", 250)];
        let share = busy_share_from_states(&states).unwrap();
        assert!((share - 0.25).abs() < 1e-9, "{share}");

        let states = vec![("IDLE", 500), ("P1", 500)];
        assert_eq!(busy_share_from_states(&states), Some(0.5));

        // No ticks at all: no share.
        assert_eq!(busy_share_from_states(&[]), None);
        assert_eq!(busy_share_from_states(&[("ELDI", 0)]), None);
    }

    #[test]
    fn busy_share_stays_in_bounds_when_counters_reset() {
        // A negative tick count is skipped, not turned into >100% busy.
        let states = vec![("ELDI", -10), ("P1", 20)];
        assert_eq!(busy_share_from_states(&states), Some(1.0));
    }
}

/// How long a thermal-pressure reading stays fresh. Pressure changes
/// over tens of seconds, so asking `pmset` on every sample (every two
/// seconds) only spends a process spawn each time.
pub const THERMAL_EVERY: std::time::Duration = std::time::Duration::from_secs(30);

/// A value read at most once per `every`, kept in between.
#[derive(Debug)]
pub struct Every<T> {
    every: std::time::Duration,
    last: Option<(std::time::Instant, T)>,
}

impl<T: Clone> Every<T> {
    pub fn new(every: std::time::Duration) -> Self {
        Self { every, last: None }
    }

    /// The kept value while it is fresh; otherwise `read` runs once and
    /// its answer is kept from `now`.
    pub fn get(&mut self, now: std::time::Instant, read: impl FnOnce() -> T) -> T {
        if let Some((at, value)) = &self.last
            && now.saturating_duration_since(*at) < self.every
        {
            return value.clone();
        }
        let value = read();
        self.last = Some((now, value.clone()));
        value
    }
}

/// Run `command` and return its standard output if it exits
/// successfully within `timeout`. A command that runs longer is killed
/// and reads as `None`, so a hung helper never stalls a sample.
pub fn output_within(
    mut command: std::process::Command,
    timeout: std::time::Duration,
) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut child = command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    let deadline = std::time::Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut out = Vec::new();
                child.stdout.take()?.read_to_end(&mut out).ok()?;
                return status.success().then_some(out);
            }
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

#[cfg(test)]
mod every_tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn a_fresh_value_is_kept_and_a_stale_one_read_again() {
        let mut reads = 0;
        let mut every = Every::new(Duration::from_secs(30));
        let start = Instant::now();
        let mut read = |value| {
            reads += 1;
            value
        };
        assert_eq!(every.get(start, || read(1)), 1);
        assert_eq!(every.get(start + Duration::from_secs(29), || read(2)), 1);
        assert_eq!(every.get(start + Duration::from_secs(30), || read(3)), 3);
        assert_eq!(reads, 2, "one read per window");
    }

    #[cfg(unix)]
    #[test]
    fn a_command_that_hangs_is_killed_at_the_deadline() {
        let mut sleep = std::process::Command::new("sleep");
        sleep.arg("10");
        let started = Instant::now();
        assert_eq!(output_within(sleep, Duration::from_millis(200)), None);
        assert!(
            started.elapsed() < Duration::from_secs(5),
            "the wait ends at the deadline"
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_quick_command_returns_its_output() {
        let mut echo = std::process::Command::new("echo");
        echo.arg("ok");
        assert_eq!(
            output_within(echo, Duration::from_secs(5)).as_deref(),
            Some(&b"ok\n"[..])
        );
    }
}
