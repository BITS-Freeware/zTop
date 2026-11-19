# Decision 0067: processes and control in the Linux app

Date: 2026-11-19
Status: accepted

## Context

L3 of zTop for Linux is the process list: search, sort, and signals,
with SIGKILL behind a confirmation. Signals must go out by the target
agent's own numbering, because macOS and Linux number STOP, TSTP and
CONT differently (the agent's `GET /v1/control` catalog exists for this
reason).

## Decision

- The process table is a section of the host detail page (decision
  0077). It reads the process list from the sample the fleet poll
  already fetches every 2 seconds, so it adds no request of its own.
  The agent's history drops process lists, so the table only shows
  the newest one.
- `ztop-desktop-core::processes` holds the search (name or user,
  case-insensitive, or a pid prefix), the sort (CPU, memory, pid, name,
  user, with the pid breaking ties in both directions so equal rows
  never swap between polls), the menu order and the confirmation rule.
  It is tested without a display.
- `ztop-core` gains `RemoteCollector::signal_catalog`. The menu lists
  the target's signals, everyday ones first (TERM, HUP, INT, STOP,
  CONT), then KILL, then the rest by number. Sending goes through the
  existing `execute_plan`, so the agent's gates (TLS, `--allow-control`,
  a token with the control right) and its refusal wording apply
  unchanged, and a refusal shows as a toast in the agent's words.
- The table shows the 30 busiest matching rows and holds still while a
  row's menu is open, so the signal goes to the row that was clicked.
  KILL asks first in an `AdwAlertDialog` that says TERM is the gentler
  choice.

## Consequences

- The end-to-end test starts a TLS agent with `--allow-control`, pairs,
  reads the catalog, sends STOP, CONT and TERM to a `sleep` the test
  owns, and checks the state in `/proc` after each. A second agent
  without `--allow-control` refuses in its own words.
- Renice is not offered yet. The agent supports it, so it can follow
  as a menu entry without protocol work.
