// SPDX-License-Identifier: MIT OR Apache-2.0
//! Each host's live state, with the same rules as the iPad and Mac app: a
//! good read is live; a failed read inside the freshness window keeps
//! the last good sample, flagged stale; past it the host is
//! unreachable. A refused token is its own state, because the fix is
//! pairing, not waiting.

use std::time::{Duration, Instant};

use ztop_core::model::Sample;

use crate::agent::ReadError;

/// A sample older than this stops counting as live.
pub const STALE_AFTER: Duration = Duration::from_secs(10);
/// How often every host is read.
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostState {
    /// No read has finished yet.
    Connecting,
    Live,
    Stale,
    Unreachable,
    /// The agent refused the token, or there is none: pair first.
    NeedsPairing,
}

impl HostState {
    /// The word the card shows.
    pub fn label(self) -> &'static str {
        match self {
            Self::Connecting => "connecting",
            Self::Live => "live",
            Self::Stale => "stale",
            Self::Unreachable => "unreachable",
            Self::NeedsPairing => "needs pairing",
        }
    }
}

/// What the overview knows about one host.
#[derive(Debug, Clone)]
pub struct HostLive {
    pub state: HostState,
    /// The last good sample, kept through stale and unreachable.
    pub sample: Option<Sample>,
    pub last_good: Option<Instant>,
    /// The last failure's wording, cleared by a good read.
    pub error: Option<String>,
}

impl Default for HostLive {
    fn default() -> Self {
        Self {
            state: HostState::Connecting,
            sample: None,
            last_good: None,
            error: None,
        }
    }
}

impl HostLive {
    /// Folds one read's outcome in, at time `now`.
    pub fn apply(&mut self, result: Result<Sample, ReadError>, now: Instant) {
        match result {
            Ok(sample) => {
                self.state = HostState::Live;
                self.sample = Some(sample);
                self.last_good = Some(now);
                self.error = None;
            }
            Err(ReadError::NeedsPairing) => {
                self.state = HostState::NeedsPairing;
                self.error = Some(ReadError::NeedsPairing.to_string());
            }
            Err(error) => {
                self.error = Some(error.to_string());
                self.state = match self.last_good {
                    Some(at) if now.duration_since(at) < STALE_AFTER => HostState::Stale,
                    _ => HostState::Unreachable,
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn failed() -> Result<Sample, ReadError> {
        Err(ReadError::Failed("connection refused".into()))
    }

    #[test]
    fn a_failure_inside_the_window_is_stale_and_past_it_unreachable() {
        let t0 = Instant::now();
        let mut live = HostLive::default();
        assert_eq!(live.state, HostState::Connecting);

        live.apply(Ok(crate::blank_sample()), t0);
        assert_eq!(live.state, HostState::Live);
        assert!(live.error.is_none());

        live.apply(failed(), t0 + Duration::from_secs(4));
        assert_eq!(live.state, HostState::Stale);
        assert!(live.sample.is_some(), "stale keeps the last good sample");
        assert_eq!(live.error.as_deref(), Some("connection refused"));

        live.apply(failed(), t0 + STALE_AFTER);
        assert_eq!(live.state, HostState::Unreachable);
        assert!(
            live.sample.is_some(),
            "unreachable still shows the last values"
        );

        live.apply(Ok(crate::blank_sample()), t0 + Duration::from_secs(30));
        assert_eq!(live.state, HostState::Live);
        assert!(live.error.is_none());
    }

    #[test]
    fn a_first_failure_is_unreachable_and_a_refused_token_needs_pairing() {
        let t0 = Instant::now();
        let mut live = HostLive::default();
        live.apply(failed(), t0);
        assert_eq!(live.state, HostState::Unreachable);

        live.apply(Err(ReadError::NeedsPairing), t0);
        assert_eq!(live.state, HostState::NeedsPairing);
        assert_eq!(live.state.label(), "needs pairing");
    }
}
