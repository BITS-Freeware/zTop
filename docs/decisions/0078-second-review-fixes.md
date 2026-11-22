# 0078 — Fixes from a second, automated review

## Context

Before an outside security review, the engine had a second automated
code review of the whole tree (two models, about a thousand files). It
listed 47 findings. Each was checked against the code; 11 were real
bugs, about ten were small documentation or CI mismatches, and the rest
were wrong. This records what changed and why.

## Decision

Each fix comes with a regression test where the behaviour can be tested
on CI.

- **Signals go to the process the menu was opened on** (desktop). A
  row's signal menu used to read its pid when the click was handled,
  after closing the menu had already redrawn the rows, so a refresh
  while the menu was open could send TERM, STOP, HUP or INT to the
  process that had moved into that row. The table now takes the target
  when the menu opens. The test runs the table on a virtual display:
  `desktop/scripts/check.sh` runs the tests under `xvfb-run` with
  `ZTOP_TEST_DISPLAY=1`; elsewhere the test says it skipped.
- **macOS GPU energy keeps one sample alive.** Each refresh released
  nothing, leaking a CoreFoundation dictionary every two seconds for
  the agent's life; it now releases the sample it replaces, the
  collector releases its setup on drop, and a failed setup releases
  what it created.
- **macOS thermal pressure** is read at most every 30 seconds
  (`power::Every`), and `pmset` is killed after two seconds
  (`power::output_within`), so a hung helper never stalls a sample.
- **macOS open files show their paths.** The vnode flavor asked for had
  no path in it (`PROC_PIDFDVNODEPATHINFO`, 2, has one), and the path
  was read from the end of the buffer rather than the end of the record
  the call filled. The new test opens a temporary file and finds its
  path, on Linux and macOS.
- **`--delay`** refuses NaN, infinity and values too large for a
  `Duration` with the usual error instead of panicking.
- **TLS error statuses.** Over TLS only 401 was checked; every other
  error status was parsed as an envelope. 429 now says to try again and
  any other status of 400 or more is an error.
- **A flaky test.** A "wrong" pin made by appending `00` to the real
  pin's first 62 characters equalled the real pin about once in 256
  runs; it now changes the first character.
- **Removing a host clears its alerts** (desktop): the engine forgets
  the host and its notifications are withdrawn.
- **Temperature charts** draw one line per sensor label, the hottest
  reading, even when sensors sharing a label are not adjacent by heat.
- **Job control works on a traced process.** The tracer swallowed
  SIGSTOP as if it were its own and resumed group-stops. Under
  `PTRACE_SEIZE` the tracer never uses SIGSTOP, so a SIGSTOP is now
  delivered, and a group-stop is resumed with `PTRACE_LISTEN`, which
  keeps the process stopped until SIGCONT.
- **The state directory is checked after it is created**, whoever made
  it: it must be a directory owned by this user (or the user is root)
  and is tightened to 0700. Before, a directory that appeared between
  the existence probe and the create skipped the tightening.
- **Rust 1.94 is the declared minimum** (`rust-version`, both
  workspaces), matching what `sqlx` 0.9 needs (decision 0073). The
  lints that raising it turns on were fixed on Linux and on macOS: a
  `cargo clippy --target aarch64-apple-darwin` from Linux works when
  `ring`'s C build is stubbed out, since lints never link.
- Smaller fixes: the packaging workflow runs for pull requests that
  touch the crates or `Cargo.lock`; the x86_64 fixture recorder rebases
  before it pushes, as the aarch64 one does; `check-arch.sh` treats `;`
  lines as systemd comments and finds packages of any compression;
  `build-apt-repo.sh` says its output must be uploaded without deleting
  older packages; an alert seed's name may contain `|`; the NVML symbol
  helper checks the size it copies; and several comments and decisions
  now say what the code does.

## Consequences

- The findings the review got wrong are listed in the review's own
  record, outside this repository, with the reason for each.
- The review skipped files that look like they hold secrets (the token
  and keyring code, the pairing decisions) and the two largest source
  files. The outside review covers those directly.
