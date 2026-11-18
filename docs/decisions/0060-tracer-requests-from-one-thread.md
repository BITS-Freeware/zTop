# 0060 — Every ptrace request from the tracer thread

Date: 2026-11-18

## Context

The syscall tracer (decision 0030) never recorded a line on any Linux
host, and closing it left the traced process frozen in a tracing stop
(state `t`) until ztop exited. Two bugs combined:

1. `trace::start` called `PTRACE_SEIZE` on the caller's thread (the
   TUI's), and every later request (`PTRACE_INTERRUPT`,
   `PTRACE_GET_SYSCALL_INFO`, `PTRACE_SYSCALL`, `PTRACE_DETACH`) came
   from the tracer thread it spawned. Linux binds a tracee to the one
   thread that attached and answers every other thread with `ESRCH`.
   The first interrupt still parked the tracee, so nothing ever
   resumed or detached it.
2. The seize passed no options. Without `PTRACE_O_TRACESYSGOOD` a
   syscall stop reports as a plain `SIGTRAP`, and
   `PTRACE_GET_SYSCALL_INFO` answers `PTRACE_SYSCALL_INFO_NONE` for
   every stop, so even a correctly threaded tracer would record
   nothing.

Decision 0040 read the resulting silence on CI as runners that "seize
but never deliver a stop", and made the live tests skip on silence.
The same silence happened on every Linux host; the skip hid the bug.

## Decision

- The tracer thread does the seize itself (with
  `PTRACE_O_TRACESYSGOOD`) and reports the outcome back over a
  channel before `start` returns, so attach errors still reach the
  status line.
- `Drop` only raises the stop flag. The tracer thread detaches: from
  the stop it is parked in, or, when the tracee is running, after its
  own `PTRACE_INTERRUPT` and a bounded wait for that stop. The detach
  passes through a pending real signal instead of swallowing it.
- Waits stay bounded (decision 0040) but back off: the first rounds
  only yield, then sleep 1 ms doubling to 20 ms. A flat 20 ms sleep
  per round cost the tracee about 40 ms per syscall.
- The live tests no longer skip on silence; they still skip when the
  attach itself is refused. They assert on `wait4`, which the traced
  shell loop really makes (its `nanosleep` runs in the forked,
  untraced `sleep`). New regression tests: a closed trace leaves the
  process running, a tracee inside a long syscall detaches cleanly,
  and a process can be traced twice from different threads.

## Consequences

- `s` works on Linux and Esc lets the process go.
- If a CI runner really does refuse stops, the trace tests now fail
  visibly instead of skipping; that is the intended signal.
- Decision 0040's bounded waits and CI timeouts stay; its diagnosis
  of the runners is superseded by this record.
