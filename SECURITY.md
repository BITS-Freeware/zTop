# Security policy

## Supported versions

| Version | Supported |
| --- | --- |
| 0.1.x | yes |

## Reporting a vulnerability

Use GitHub's private vulnerability reporting for this repository (the
Security tab's "Report a vulnerability" button). Do not open a public issue
for anything exploitable. Reports are read and fixed by the maintainer; you
will be credited in the changelog if you want to be.

## What zTop guarantees

- There is no remote command execution and no shell, by design — not a
  disabled feature, an absent one. The agent and the hub are read-only by
  default; the one way to act on a machine is sending a signal from a fixed
  catalog to, or renicing, one named process, and only when the agent runs
  with `--allow-control` and the token holds the `control` right.
- The agent and hub bind to the loopback interface until explicitly
  configured otherwise. Serving beyond loopback requires TLS and pairing;
  without TLS, other addresses are refused.
- Remote clients need a token derived from pairing. Tokens are revocable and
  carry only the rights they were granted; signals and renice are refused
  unless the agent config enables them and the token has that right.
- The agent runs unprivileged and never asks for root. Data it may not read
  is shown as unavailable, never faked.

## Out of scope

- Attackers who already have root on the monitored machine or the hub host,
  or physical access to them.
- A client device the user paired being compromised.
- Unsupported versions.

## Notes for operators

- On Linux, run the agent as its own unprivileged user (the `.deb` package
  creates `ztop` and runs its system service as that user).
- Treat the hub host as trusted with everything its agents can see.
- The hub's SQLite history contains process names; handle the file with
  normal file-permission hygiene.
