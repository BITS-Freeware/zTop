// SPDX-License-Identifier: MIT OR Apache-2.0
//! The zTop Linux app's logic, kept apart from GTK so it is tested
//! without a display: the saved hosts, the token store, each host's
//! live state, the numbers a fleet card shows, a host's recent history
//! for its detail page, and the calls to an
//! agent or hub through `ztop-core`'s own client.

pub mod agent;
pub mod freshness;
pub mod history;
pub mod hosts;
pub mod processes;
pub mod summary;
pub mod tokens;

pub use agent::{NewHost, ReadError};
pub use freshness::{HostLive, HostState};
pub use history::History;
pub use hosts::{HostRecord, HostRegistry};
pub use summary::CardSummary;
pub use tokens::{FileTokenStore, TokenStore};

use std::path::PathBuf;

/// Where the app keeps its files: `$ZTOP_DESKTOP_HOME` when set (tests
/// and recorded runs isolate themselves with it), else
/// `$XDG_CONFIG_HOME/ztop-desktop`, else `~/.config/ztop-desktop`.
pub fn config_dir() -> Option<PathBuf> {
    config_dir_with(
        std::env::var_os("ZTOP_DESKTOP_HOME"),
        std::env::var_os("XDG_CONFIG_HOME"),
        std::env::var_os("HOME"),
    )
}

/// [`config_dir`] with its inputs passed in, for tests.
pub fn config_dir_with(
    explicit: Option<std::ffi::OsString>,
    xdg: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    if let Some(dir) = explicit.filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(dir));
    }
    // The XDG spec ignores a relative XDG_CONFIG_HOME.
    if let Some(xdg) = xdg.map(PathBuf::from).filter(|d| d.is_absolute()) {
        return Some(xdg.join("ztop-desktop"));
    }
    home.filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(h).join(".config").join("ztop-desktop"))
}

/// A sample for tests with every figure the card reads unreported:
/// a trimmed real agent snapshot (`fixtures/sample.json`, fake names)
/// with its cores, memory and uptime blanked.
#[cfg(test)]
pub(crate) fn blank_sample() -> ztop_core::model::Sample {
    use ztop_core::model::Metric;
    let mut sample: ztop_core::model::Sample =
        serde_json::from_str(include_str!("fixtures/sample.json")).expect("the fixture sample");
    sample.hostname.clear();
    sample.cpu.busy_percent.clear();
    sample.memory.total = Metric::unavailable(sample.memory.total.unit);
    sample.memory.used = Metric::unavailable(sample.memory.used.unit);
    sample.uptime = Metric::unavailable(sample.uptime.unit);
    sample
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_config_dir_prefers_the_explicit_home_then_xdg_then_home() {
        let some = |s: &str| Some(std::ffi::OsString::from(s));
        assert_eq!(
            config_dir_with(some("/tmp/a"), some("/x"), some("/home/alice")),
            Some(PathBuf::from("/tmp/a"))
        );
        assert_eq!(
            config_dir_with(None, some("/x"), some("/home/alice")),
            Some(PathBuf::from("/x/ztop-desktop"))
        );
        assert_eq!(
            config_dir_with(some(""), some("relative"), some("/home/alice")),
            Some(PathBuf::from("/home/alice/.config/ztop-desktop"))
        );
        assert_eq!(config_dir_with(None, None, None), None);
    }
}
