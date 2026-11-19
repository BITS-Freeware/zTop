# 0070 — The tokens file is written whole, and a corrupt one is loud

## Context

Audit finding M3: `Auth::save` truncated `tokens.json` and then wrote
it, so a crash in between left a partial file. On the next start
`Auth::at(...).unwrap_or_default()` fell back to an in-memory store
with no path: every client was unpaired, new pairings were never saved,
and nothing was printed. A tokens file the agent could not read at all
(permissions) was treated as no file.

## Decision

- `save` writes a 0600 temporary file beside the real one, syncs it and
  renames it over the target (`auth::write_atomically`).
- A tokens file that does not parse is moved aside to
  `tokens.json.corrupt-<unix ms>` with a line on stderr, and the agent
  starts with no tokens at the same path, so pairings persist again.
- A file that exists but cannot be read is an error, and the agent and
  hub refuse to start, naming the file.

## Consequences

- A crash during a save leaves the old file or the new one.
- Recovering from a corrupt file revokes every client (they pair again),
  which fails safe, and the old file stays for a look.
- A permissions mistake stops the service visibly instead of running it
  without persistence.
