# 0062 — Hub retention that deletes, and history without process lists

Date: 2026-11-18

## Context

The hub promised a 24-hour history (decision 0048) and kept everything
forever:

- The retention task passed an absolute cutoff (`now - 24h`) to
  `Store::prune`, which takes an age and computes `now - age` itself.
  The resulting cutoff was a day after 1970, so the prune deleted
  nothing. Its unit test called `prune` with an age directly and never
  ran the task's arithmetic.
- Each polled sample was stored verbatim, process list included. One
  snapshot from a 142-process host was 66 KB; at one poll every two
  seconds that is about 2.9 GB per agent per day.
- `GET /v1/history?host=` without `seconds` loaded every stored row
  into memory and serialized it in one answer.

The agent's own ring had the same process-list problem and was fixed
before 0.1.0; the hub was not.

## Decision

- `HubState::prune_expired` is the retention task's one call
  (`store.prune(RETENTION_MS)`, an age), and the test runs that exact
  call. `prune` saturates instead of going negative, and a failed prune
  is logged rather than dropped.
- Stored rows drop the process list. The newest full sample per agent
  stays in memory (`HubState::latest`) and the snapshot route serves
  it, falling back to the store (no process list) only between a hub
  restart and that agent's first poll.
- History windows are capped at `MAX_HISTORY_SECONDS` (one hour, what
  the app asks for); a request without `seconds` gets the cap, and the
  answer's `seconds` names the window actually served.

## Consequences

- An existing `history.db` is cut to the last 24 hours on the first
  prune after upgrade. SQLite keeps the file at its old size and reuses
  the freed pages; `sqlite3 history.db 'VACUUM'` with the hub stopped
  returns the space.
- History rows are the same shape the agent's ring serves (no
  processes), so clients see one shape from both.
- The hub still files rows under the agent's timestamp while pruning by
  its own clock; a skewed agent clock shifts its window. That is a
  separate finding, left for its own change.
