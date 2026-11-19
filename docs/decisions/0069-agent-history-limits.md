# 0069 — The agent's history answers are bounded

## Context

Audit finding M2: `GET /v1/history` with no `seconds` returned the
whole 3,600-sample ring, cloned under the history lock and serialized
on the async runtime, and any number of requests could do that at
once. The threat model said history-range limits were enforced; none
were. On plain loopback, which needs no token, any local user could
call it in a loop.

## Decision

- A window is at most `MAX_HISTORY_SECONDS` (3,600), the same bound
  the hub uses (decision 0062). A request without `seconds` gets that
  maximum, and the answer echoes the window it used.
- At most two history answers are built at once. A third request gets
  429 with `Retry-After: 1` instead of queueing another copy.
- The copy and the serialization run on a blocking thread, so a large
  answer does not stall the runtime that serves every other route.

## Consequences

- One answer is bounded by an hour of samples without process lists,
  and the work in flight by two of them.
- A client that asks for history from several screens at once may see
  429 and should retry; the app's fetch loops already retry on their
  next tick.
- The hourly cost of an app that re-reads a full hour every few seconds
  stays the app's to reduce (fetch the delta since its last sample).
