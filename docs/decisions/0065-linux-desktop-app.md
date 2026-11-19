# 0065 — A Linux desktop app in Rust, GTK4 and libadwaita

## Context

The zTop app runs on iPad and Mac. On a Linux workstation the fleet
could be watched only through `ztop connect`, one host at a time in a
terminal. A native Linux desktop app closes that gap. It is open source
and lives in this repository, next to the agent and hub it talks to.

`ztop-core` already holds a complete client: `RemoteCollector` (pinned
TLS reads of `/v1/snapshot`), `pair_with`, the control-plan parser and
the data model. It depends on libc, rustls, serde and sha2, nothing
GUI- or runtime-specific.

## Decision

1. **A workspace of its own.** `desktop/` holds `ztop-desktop-core`
   (saved hosts, token store, freshness, card figures, agent calls; no
   GTK, tested without a display) and `ztop-desktop` (the window and
   dialogs). It is not a member of the root workspace, so building and
   testing `ztop` never needs the GTK development packages. Both crates
   use `ztop-core` by path: the protocol, pairing and pinning are this
   repository's own code, never a copy.
2. **The same rules as the iPad and Mac app.** Hosts are read every
   2 s; a failed read inside 10 s of the last good one is stale (the
   last values stay on screen), past it unreachable. A refused token is
   its own state, "needs pairing", with a Pair button on the card.
   Tokens are never in `hosts.json`.
3. **Tokens in the Secret Service, through libsecret.** One item per
   host in the default collection, schema `dev.ztop.Desktop.Token`,
   attribute `host-id`. With no Secret Service, tokens go to 0600 files
   in a 0700 directory, and a banner says so. The pure-Rust
   `secret-service` crate was tried first and dropped: its zbus stack
   is flagged unmaintained (RUSTSEC-2026-0370 to 0372), so
   `cargo deny` fails on it. libsecret is the desktop's own C library,
   already present wherever a keyring runs.
4. **Threads.** The client blocks, so every read, pairing and keyring
   call runs through `gio::spawn_blocking` and returns to the GTK main
   thread as a future. At most one read per host is in flight.
5. **Recorded runs.** `ZTOP_DESKTOP_SEED` pairs hosts at start,
   `ZTOP_DESKTOP_SNAPSHOT` draws the window into a PNG through GTK's
   own renderer and prints each host's state, and `ZTOP_DESKTOP_HOME`
   isolates the config. `desktop/scripts/recorded-run.sh` runs two TLS
   agents with fake names, a throwaway gnome-keyring in its own D-Bus
   session and Xvfb, and fails unless both hosts read live and the
   tokens are in the keyring.
6. **CI.** A Desktop workflow on GitHub-hosted Ubuntu 24.04 (GTK 4.14,
   libadwaita 1.5, the versions the crates' feature flags target) runs
   `desktop/scripts/check.sh`: fmt, clippy with warnings denied,
   `cargo deny`, the tests (live agents and the keyring), and the
   recorded run.

New dependencies: `gtk4` 0.11 and `libadwaita` 0.9 (MIT), `libsecret`
0.9 (MIT), `getrandom` (already used here). The GTK, libadwaita and
libsecret C libraries are LGPL system libraries, linked dynamically and
not vendored.

## Consequences

- The app needs GTK 4.14, libadwaita 1.5 and libsecret 0.21 or newer:
  Ubuntu 24.04, Debian 13, Fedora 40 and later.
- An agent answers 503 until its sampler has produced a first sample,
  and `RemoteCollector` reports that as "no understandable envelope".
  A freshly started agent can show unreachable for one read. The live
  tests wait it out; a clearer client error is a follow-up here.
- Later milestones (host detail, processes, discovery and alerts,
  packaging) are listed in ROADMAP.md.
