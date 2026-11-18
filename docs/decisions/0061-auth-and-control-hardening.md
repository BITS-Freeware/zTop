# 0061 — Auth and control hardening

Date: 2026-11-18

## Context

A review of 0.1.0 found three gaps in the agent's and hub's auth and
control paths:

1. `POST /v1/control` passed each `u32` pid to `kill` as a `c_int`.
   Pid 4294967295 became `kill(-1, sig)`, every process the agent's
   user may signal; pid 0 meant the agent's own process group, and
   pids past `i32::MAX` whole process groups. `setpriority`,
   `ioprio_set` and `sched_setaffinity` read pid 0 as the agent itself.
   Any signal number and any renice delta went through.
2. `GET /v1/tokens` and `DELETE /v1/tokens/{id}` sat behind the same
   check as `/v1/snapshot` on both the agent and the hub, so a
   read-only token could list every client and revoke the owner's
   control token. Plain loopback serving checks nothing, so any local
   user could.
3. Pairing codes were six symbols from 31 (about 887 million), a wrong
   guess did not burn the code, and nothing limited the rate (a debug
   build answered about 7,400 guesses a second). The shipped launchd
   plist wrote standard output, code included, to `/tmp`, readable by
   every local user; Homebrew's `var/log` file is created with the
   default umask too.

## Decision

- **Control checks live in `ztop_core::control::check`**, in front of
  every `apply_one`, so the TUI and the agent share them: pids must be
  in `1..=i32::MAX`; signals must be in `common_signals()` or 0 (the
  permission probe); a renice step is at most 39 (the width of the
  nice range); I/O priorities must be picker values and affinity masks
  non-empty.
- **A `manage` right.** Token list and revoke need a token carrying
  `manage`, on every transport, including unenforced loopback. Only a
  code printed by `--pair --allow-manage` grants it; the shipped
  services do not pass the flag. The agent and hub share one gate
  (`refuse_unless_manager`) and one pairing answer (`pair_response`).
- **Pairing codes resist guessing.** Eight symbols (31^8, about
  8.5 * 10^11), drawn without modulo bias and compared in constant
  time. Five wrong guesses burn the code; an attempt within a second of
  a wrong guess answers 429 and is not counted. A guessing client gets
  five tries per printed code.
- **Codes never land in a log others can read.** Before printing a
  code into a regular file, the server narrows that file to its owner
  (keeping the owner's bits). If another user owns it, the code goes to
  `pairing-code` (mode 0600, created fresh) in the state directory and
  the log names only the path. Terminals and pipes (the Mac app reads
  the hub's code from a pipe) are unchanged. The launchd plist logs to
  `~/Library/Logs/ztop-agent.log`, filled in by `sed` at install since
  launchd needs absolute paths.
- The agent crate now depends on `libc` (already a workspace
  dependency, used by core) for `geteuid`.

A per-address limit was not added: with five guesses per code it would
change the odds by nothing measurable, and it would need the peer
address threaded through the TLS accept path.

## Consequences

- A wrong code burns after five tries even for the owner; the person
  restarts pairing. A service started with `--pair` still opens a new
  code at each start, now worth five guesses.
- Clients that listed or revoked tokens with a read token now get 403.
  No shipped client did.
- The output format of `Pairing code: <code>` is unchanged on
  terminals and pipes, so the app and the packaging CI parse it as
  before; codes are two characters longer.
- Pairing on demand against a running agent (`ztop pair`) stays a
  possible follow-up; it would let services drop `--pair`.
