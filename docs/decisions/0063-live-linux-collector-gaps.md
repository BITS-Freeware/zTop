# 0063 — Close the gaps between the live and recorded Linux paths

Date: 2026-11-18

## Context

The Linux collector has two assemblers over shared parsers:
`sample_from_fixture` (replay, which the fixture tests cover) and
`LinuxCollector::sample` (live, which the agent ships). The live one had
drifted:

1. It never read `/proc/<pid>/cmdline`; only the replay path did. On a
   live host every process had no command line.
2. `/proc/mounts` was keyed by its source (`/dev/vda`) and looked up by
   the `/proc/diskstats` name (`vda`), so no disk ever had a mount point
   and `statvfs` never ran: every disk showed 0 bytes. The fixture's
   only block mount is `/dev/root`, so the replay path hid it too.
3. `parse_proc_stat` refused core numbers with gaps. `/proc/stat` lists
   only online CPUs, so turning SMT off or unplugging a vCPU failed
   every sample, the agent answered 503 forever, and `tick` dropped the
   error without a word.

`docs/parity.md` checked the command-line and disk items on the strength
of fixture tests alone.

## Decision

- The live loop reads `cmdline` (lossily, so one non-UTF-8 argument
  does not lose the line) through the same `parse_pid_cmdline`.
- `parse_proc_mounts` keys by the kernel device name: only `/dev/...`
  sources, last path component, first mount per device wins (so `/`
  beats a later bind mount), octal escapes decoded.
  `parse_proc_mounts_with` takes the naming as a function; the live
  collector resolves links with `canonicalize` (`/dev/mapper/*` →
  `dm-0`, `/dev/disk/by-*` → `sda1`) and an alias with no node
  (`/dev/root`) through the mount point's device number in
  `/sys/dev/block/MAJ:MIN`.
- Cores keep their numbers. A core missing below the highest listed one
  is offline: zeroed times and a new `Cpu.offline` list (serde default,
  omitted from JSON when empty, so the protocol is unchanged for hosts
  with every core online and older clients ignore it). A core offline
  in this sample or the previous one reports no busy share.
- The agent's sampler logs a failure when it starts or changes, and the
  recovery, instead of discarding it.
- New live tests (`crates/core/tests/linux_live.rs`) run the live
  assembler on whatever Linux host runs CI, with universal bounds: this
  process has a command line naming the test binary, and every disk
  whose device is mounted by a `/dev` path has a mount point and a
  capacity.

## Consequences

- The command-line and per-disk parity items are now backed by live
  tests as well as fixtures.
- Collapsing the two assemblers into one over a file-reader trait (so
  every fixture test exercises the live path) remains the structural
  fix; this record closes the known gaps without that refactor.
- The TUI and the app draw an offline core as a zero bar; showing it as
  unavailable is a client-side follow-up that `Cpu.offline` now allows.
