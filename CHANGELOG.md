# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Security

- The syscall tracer releases the traced process on close, and every ptrace request comes from one thread.
- Control refuses pid 0, group pids and signals outside the catalog; listing and revoking tokens needs the new `manage` right; pairing codes are eight symbols, burned by five wrong guesses and spaced a second apart.
- The agent and the hub keep serving through failed accepts, with deadlines for the TLS handshake, the request head and the answer, and a connection cap.
- The protocol client parses responses as bytes (a crafted response could panic it), reads at most 32 MiB, and gives each fetch one overall deadline.
- Agent history answers cover at most an hour, and at most two are built at once.
- The TLS key, the state directory and the hub's history database are private (0600 and 0700) from the moment they exist; the tokens file is written atomically, and a corrupt one is moved aside loudly instead of silently disabling persistence.
- The hub's bundled SQLite moves past RUSTSEC-2026-0459 (sqlx 0.9), and CI now checks advisories.

### Fixed

- Hub retention deletes rows past a day, history no longer stores process lists, and windows, pruning and the latest sample go by the hub's own clock.
- The live Linux collector reads command lines, finds mounted disks, survives offline CPUs and reads only the sensor and battery files it uses.
- A failed mDNS start no longer crashes the agent.
- The .deb installs under `/usr`, runs the agent as a sandboxed system service for the `ztop` user, and removes only what it created.

## [0.1.0] - 2026-11-18

First public release.

### Fixed

- The agent's history ring no longer keeps a full process list for every sample, which made its memory grow for an hour after start.

### Added

- **Terminal monitor** (`ztop`): a full-screen TUI with header meters (per-core CPU bars, memory and swap bars, load average, uptime, task and thread counts, and battery where present), a process table with every column, mode and action in the parity list (tree view, sorting by any column, filter, incremental search, follow a process, tag and act on several, send any signal, renice, user filter, thread display, I/O columns, and setup screens for meters, columns and colors, saved to a config file), and more than both `htop` and `asitop`: disk and network I/O panels, per-disk usage, temperatures and fans, GPU on Linux (NVIDIA through NVML loaded at runtime, AMD through sysfs), and container/cgroup awareness on Linux.
- **Apple Silicon view** (asitop parity): E-cluster and P-cluster frequency and utilization, per-core frequency, GPU frequency and utilization, ANE power, CPU, GPU and package power with history charts and peak values, memory and swap use, and thermal pressure.
- **Agent** (`ztop agent`): serves metrics to the zTop app, a hub or `ztop connect` over a versioned protocol (a WebSocket stream of samples and an HTTP endpoint for a snapshot, the process list and history), with TLS 1.3, pairing, and token-based authentication.
- **Hub** (`ztop hub`): collects from many agents and serves them as one, with longer history in SQLite.
- **Packaging**: Homebrew formula, .deb packages for amd64 and arm64 with a systemd unit and a dedicated user, and a man page.
- **Fuzzing**: 17 fuzzing targets for all network-facing parsers, smoke-run in CI on every push.
- **Accessibility**: VoiceOver labels for cards, charts, and meters.

### Changed

- The agent's own cost is part of the product: at a 1-second interval, the agent uses under 1% of one core and under 30 MB resident memory on a typical host.

### Security

- The agent is read-only by default, listens only on localhost until configured, and needs pairing before any remote client can read from it.
- TLS 1.3 with a self-signed certificate per agent.
- Pairing shows a short code on the agent; the client sends it once, receives a long-lived token, and pins the agent's certificate.
- Tokens can be listed and revoked on the agent.
- Sending signals or renicing from the app is off unless the agent's config turns it on, and then only for tokens granted that right.
- There is no remote shell or command execution, ever.
