# 0071 — The hub goes by its own clock

## Context

Audit finding M4: the hub filed each row under the agent's
`timestamp_ms`, but computed history windows and the retention cutoff
from its own clock, and picked the latest row by the agent's
timestamp. An agent with a skewed clock got wrong windows (a day
behind, its fresh samples were already "a day old" and pruned), and an
agent whose clock stepped backward pinned an older sample as the
latest.

## Decision

- Migration `0002_received_ms` adds `received_ms`, the hub's clock
  when the poll answered, with its own index. Rows from before the
  migration take their agent time as receive time.
- Windows, the retention prune and "latest" go by `received_ms`, with
  the row id as the tie-break. The agent's timestamp stays in the row
  and in the payload, unchanged, so clients still see when the agent
  took the sample.

## Consequences

- History served by the hub is ordered by arrival, which is the order
  the hub polled in; a client that plots by the payload's timestamp
  sees the agent's own clock, skew included, as before.
- One more integer per row.
