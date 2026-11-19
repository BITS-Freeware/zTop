# 0072 — ztop's state is private from the moment it exists

## Context

Audit finding M6: `agent-key.pem` was written with the default umask
and only then changed to 0600, leaving a window where other local
users could read the agent's TLS key. The state directory was created
0755, and the hub's `history.db`, which holds every agent's samples,
was created with default permissions.

## Decision

- `ztop_core::paths::create_private_dir` creates every missing level
  0700, and tightens an existing directory to 0700 when it is ztop's
  state directory or inside it. The agent, the hub, the client's saved
  host trust, the pairing notice and the TUI config all use it.
- The TLS key and certificate are written through
  `auth::write_atomically`: a 0600 temporary file, synced, renamed.
- The hub creates `history.db` empty and 0600 before SQLite opens it,
  so SQLite's `-wal` and `-shm` files inherit that mode, and it takes
  group and other access away from an existing database.

## Consequences

- An install from 0.1.0 gets its state directory and database
  tightened on the next start; a key written by 0.1.0 already ended up
  0600.
- Parents outside ztop's state directory (`~/.config`, a custom
  `--db` location) are created 0700 when missing but never changed
  when they exist.
