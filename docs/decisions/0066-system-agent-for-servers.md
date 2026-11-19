# 0066 — A system agent for servers

## Context

The package's agent is a per-person user service (decision 0074): it
starts with that person's session and serves their view of the
machine. A server has no session. Running the user unit there means
lingering a login account and keeping the agent's certificate and
tokens in a person's home, and the shipped way to read the pairing code
is that service's log. Servers usually sit on a private network
(Tailscale, WireGuard) whose address appears only after that network's
daemon starts, and a hub should read them over that network only.

## Decision

- The `.deb` also ships a system unit,
  `/usr/lib/systemd/system/ztop-agent.service`, disabled. It runs
  `ztop agent --tls --no-advertise` as the unprivileged `ztop` system
  user (created by `systemd-sysusers` from
  `/usr/lib/sysusers.d/ztop.conf`, or `useradd` without it), with
  `StateDirectory=ztop`: the certificate and token hashes live in
  `/var/lib/ztop`, mode 0700.
- It binds 127.0.0.1 until `/etc/default/ztop-agent` (a conffile) names
  another address in `ZTOP_AGENT_ARGS`, so turning it on never exposes
  the agent by accident. It is ordered after `network-online.target` and
  `tailscaled.service` and restarts every five seconds without a start
  limit, so a bind to an address that isn't up yet succeeds once it is.
- The unit never pairs and never grants control or management; the
  package check and `scripts/check.sh` refuse a unit or options file
  that does. The unit drops every capability and confines the agent
  (`ProtectSystem=strict`, `ProtectHome=read-only`, a system call
  filter, `MemoryDenyWriteExecute`, 128 MB memory cap). It keeps
  `/dev` visible, because the disk collector resolves
  `/dev/mapper` and `/dev/disk/by-*` links (decision 0063);
  `DevicePolicy=closed` still refuses opening devices.
- Pairing is `/usr/lib/ztop/pair-system-agent`, run as root: it starts
  a second agent as `ztop` on loopback with `--pair`, its output in a
  file only root reads (so the agent's own leak check, decision 0061,
  judges it private), trades the code over loopback with `ztop connect
  --once`, stops that agent, restarts the service if it runs, and
  prints `{"pin","token"}`. The code never reaches the journal or the
  network, and the token reads only.
- Upgrades don't restart the system agent; it runs the new binary the
  next time it restarts. Removing the package disables and stops it;
  purging deletes `/var/lib/ztop`.

## Consequences

- A server joins a hub with three commands (set the address, pair,
  enable), documented in `/usr/share/doc/ztop/README.server`.
- `packaging/check-deb.sh` checks the unit's user and flags, the
  `ztop` user, the private state directory, a pairing through the
  helper, and that the paired token reads a snapshot and is refused
  control and token listing. CI containers have no systemd, so the
  check runs the agent the way the unit does instead of through
  systemd, and runs `systemd-analyze verify` only where it exists.
- Upgrading from the 0.1.0 package, whose `ztop` user nothing used,
  reuses that user.
- Revoking every client means deleting `/var/lib/ztop/ztop/tokens.json`
  and restarting; revoking one still needs a token with the `manage`
  right, which the system agent never grants.
