# 0074 — Advisories run in CI, and the hub moves to sqlx 0.9

## Context

Audit finding M8: `scripts/check.sh` ran `cargo deny check licenses
bans sources`, while the threat model says advisory checks run too.
Running advisories failed on RUSTSEC-2026-0459 in `libsqlite3-sys`
0.30.1, the bundled SQLite before 3.54 (uninitialized page-cache memory
when a database is opened with a `-wal` file of a different page
size). The audit expected the fix to need a newer `sqlx` and suggested
ignoring the advisory meanwhile.

## Decision

- The workspace moves from `sqlx` 0.8 to 0.9 (0.9.1 today), which
  accepts `libsqlite3-sys` 0.40 and its SQLite after 3.54. The hub's
  code needed no change; its tests pass unchanged.
- `check.sh`, and therefore CI, runs `cargo deny check advisories
  licenses bans sources`. Nothing is ignored.
- `sqlx` 0.9 needs Rust 1.94 to build. The workspace's own
  `rust-version` stays 1.85: raising it turns on lints keyed to the
  minimum version (clippy's `collapsible_if` with let chains) across
  macOS-only code that cannot be linted from Linux, and that change
  belongs in its own commit, checked on both platforms.
  CONTRIBUTING.md names 1.94 as the toolchain to build with.

## Consequences

- A new advisory against any dependency fails CI the day it is
  published, which is the point; a fix that has to wait is then ignored
  in `deny.toml` with its ID and a reason, never silently.
- A toolchain older than 1.94 stops at `sqlx`'s own version check.
