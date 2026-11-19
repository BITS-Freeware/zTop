// SPDX-License-Identifier: MIT OR Apache-2.0
//! Showing the pairing code without leaking it.
//!
//! The person at the machine reads the code from the server's standard
//! output: a terminal, the app's pipe, or a service manager's log file.
//! A log file is the risky one: launchd and Homebrew create theirs with
//! the default umask, readable by every local user, and anyone who reads
//! a live code can pair. So before the code is printed into a regular
//! file, the file is narrowed to its owner; when that is impossible
//! (another user owns it), the code goes to a 0600 file in the state
//! directory instead and the log only says where.

use std::io::Write;
use std::path::Path;

/// Whether a code printed to an output would stay with its owner.
#[derive(Debug, PartialEq, Eq)]
pub enum Exposure {
    /// A terminal, pipe, socket, or a regular file only its owner (this
    /// process's user) can read.
    Private,
    /// A regular file other users can read, and this process cannot fix
    /// that; the reason is kept for the log line.
    Shared(String),
}

/// Judge the output behind `file`, narrowing a regular file this user
/// owns to mode 0600 (keeping the owner's bits) when group or others
/// could read it.
#[cfg(unix)]
pub fn exposure_of(file: &std::fs::File) -> Exposure {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let metadata = match file.metadata() {
        Ok(metadata) => metadata,
        // Nothing to judge (a closed or odd descriptor): printing there
        // reaches nobody else either.
        Err(_) => return Exposure::Private,
    };
    if !metadata.file_type().is_file() {
        return Exposure::Private;
    }
    let mode = metadata.mode() & 0o7777;
    if mode & 0o077 == 0 {
        return Exposure::Private;
    }
    let me = unsafe { libc::geteuid() };
    if metadata.uid() != me {
        return Exposure::Shared(format!(
            "standard output is a log file owned by uid {} and readable by other users",
            metadata.uid()
        ));
    }
    match file.set_permissions(std::fs::Permissions::from_mode(mode & 0o700)) {
        Ok(()) => Exposure::Private,
        Err(error) => Exposure::Shared(format!(
            "standard output is a log file readable by other users, and narrowing it failed: {error}"
        )),
    }
}

#[cfg(not(unix))]
pub fn exposure_of(_file: &std::fs::File) -> Exposure {
    Exposure::Private
}

/// Show `code` to the person at the machine: on standard output when
/// that stays private, else in `fallback_dir/pairing-code` (mode 0600),
/// with only the file's location in the log. `service` prefixes the
/// notes ("ztop-agent", "ztop-hub").
pub fn announce_pairing_code(code: &str, service: &str, fallback_dir: Option<&Path>) {
    let exposure = stdout_file()
        .map(|file| exposure_of(&file))
        .unwrap_or(Exposure::Private);
    let reason = match exposure {
        Exposure::Private => {
            println!("Pairing code: {code} (valid for 5 minutes)");
            return;
        }
        Exposure::Shared(reason) => reason,
    };
    let Some(dir) = fallback_dir else {
        eprintln!("{service}: pairing code withheld: {reason}, and there is no state directory");
        return;
    };
    let path = dir.join("pairing-code");
    match write_private(&path, &format!("{code}\n")) {
        Ok(()) => println!(
            "Pairing code: withheld from this log ({reason}); read it from {} (valid for 5 minutes)",
            path.display()
        ),
        Err(error) => eprintln!(
            "{service}: pairing code withheld: {reason}, and writing {} failed: {error}",
            path.display()
        ),
    }
}

/// A handle on this process's standard output, without taking it over.
#[cfg(unix)]
fn stdout_file() -> Option<std::fs::File> {
    use std::os::fd::AsFd;
    let fd = std::io::stdout().as_fd().try_clone_to_owned().ok()?;
    Some(std::fs::File::from(fd))
}

#[cfg(not(unix))]
fn stdout_file() -> Option<std::fs::File> {
    None
}

/// Replace `path` with `text`, readable by its owner only from the
/// moment it exists.
fn write_private(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        ztop_core::paths::create_private_dir(parent)?;
    }
    // A fresh file each time: removing first means an existing file's
    // looser mode (or a planted link) is never reused.
    match std::fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)?.write_all(text.as_bytes())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("ztop-notice-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("dir");
        dir
    }

    /// Regression: launchd wrote the agent's standard output, pairing
    /// code included, to a world-readable /tmp file. A log file this
    /// user owns is narrowed to 0600 before the code is printed.
    #[test]
    fn a_world_readable_log_is_narrowed_before_the_code_is_printed() {
        let dir = scratch("narrow");
        let path = dir.join("agent.log");
        std::fs::write(&path, "").expect("log");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        let file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .expect("open");
        assert_eq!(exposure_of(&file), Exposure::Private);
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn terminals_and_pipes_are_private() {
        let null = std::fs::File::open("/dev/null").expect("null");
        assert_eq!(exposure_of(&null), Exposure::Private);
    }

    #[test]
    fn the_fallback_file_is_private_and_fresh() {
        let dir = scratch("fallback");
        let path = dir.join("pairing-code");
        std::fs::write(&path, "old").expect("old");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o666)).expect("chmod");
        write_private(&path, "abcdefgh\n").expect("write");
        let mode = std::fs::metadata(&path).expect("stat").permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(std::fs::read_to_string(&path).expect("read"), "abcdefgh\n");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
