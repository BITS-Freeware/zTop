// SPDX-License-Identifier: MIT OR Apache-2.0
//! Shared file locations: where ztop keeps its config and state.

use std::ffi::OsStr;
use std::path::PathBuf;

/// The directory for ztop's own files, or `None` without a home
/// directory.
pub fn state_dir() -> Option<PathBuf> {
    state_dir_with(
        std::env::var_os("HOME").as_deref(),
        std::env::var_os("XDG_CONFIG_HOME").as_deref(),
    )
}

/// The directory for ztop's own files, from explicit inputs so tests
/// stay hermetic (no environment mutation, which races between test
/// threads).
///
/// On macOS it is `~/Library/Application Support/ztop`; elsewhere
/// `$XDG_CONFIG_HOME/ztop` when `$XDG_CONFIG_HOME` is an absolute path
/// (per the XDG base directory spec) and `~/.config/ztop` otherwise.
pub fn state_dir_with(home: Option<&OsStr>, xdg: Option<&OsStr>) -> Option<PathBuf> {
    let home = home?;
    #[cfg(target_os = "macos")]
    {
        let _ = xdg;
        Some(PathBuf::from(home).join("Library/Application Support/ztop"))
    }
    #[cfg(not(target_os = "macos"))]
    {
        match xdg {
            Some(base) if PathBuf::from(base).is_absolute() => {
                Some(PathBuf::from(base).join("ztop"))
            }
            _ => Some(PathBuf::from(home).join(".config/ztop")),
        }
    }
}

/// Create `dir` and any missing parents, each new one mode 0700 on
/// unix, so ztop's state (tokens, keys, history) is never readable by
/// other users, not even for the moment between a create and a chmod.
/// When `dir` is ztop's own state directory or inside it, it must be
/// owned by this user (or the user must be root), and an existing one is tightened to 0700; other
/// existing parents are left alone. The check runs after the create,
/// whoever made the directory, so one that appears between a probe and
/// the create cannot skip it.
pub fn create_private_dir(dir: &std::path::Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        if is_ours(dir) {
            // Root (a `sudo` that keeps HOME) may use the user's own.
            let meta = std::fs::metadata(dir)?;
            let euid = unsafe { libc::geteuid() };
            if !meta.is_dir() || (meta.uid() != euid && euid != 0) {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    format!("{} is not a directory owned by this user", dir.display()),
                ));
            }
            let mode = meta.permissions().mode() & 0o777;
            if mode & 0o077 != 0 {
                std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        Ok(())
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

/// Whether `dir` is ztop's state directory or inside it.
#[cfg(unix)]
fn is_ours(dir: &std::path::Path) -> bool {
    state_dir().is_some_and(|state| dir.starts_with(state))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression (audit M6): the state directory was created 0755.
    #[cfg(unix)]
    #[test]
    fn private_dirs_are_created_0700_all_the_way_down() {
        use std::os::unix::fs::PermissionsExt;
        let base = std::env::temp_dir().join(format!("ztop-private-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let deep = base.join("a").join("b");
        create_private_dir(&deep).expect("creates");
        for dir in [&base, &base.join("a"), &deep] {
            let mode = std::fs::metadata(dir).expect("meta").permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "{}", dir.display());
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn without_a_home_there_is_no_state_dir() {
        assert_eq!(state_dir_with(None, None), None);
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn an_absolute_xdg_base_wins_and_a_relative_one_does_not() {
        let home = std::ffi::OsStr::new("/home-alice");
        assert_eq!(
            state_dir_with(Some(home), Some(std::ffi::OsStr::new("/tmp-alice/config"))),
            Some(PathBuf::from("/tmp-alice/config/ztop"))
        );
        assert_eq!(
            state_dir_with(
                Some(home),
                Some(std::ffi::OsStr::new("relative/not/absolute"))
            ),
            Some(PathBuf::from("/home-alice/.config/ztop"))
        );
    }
}
