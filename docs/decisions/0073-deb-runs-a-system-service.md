# 0073 — The .deb runs the agent as a system service for the ztop user

## Context

Audit finding M7: DESIGN.md promises a systemd unit that runs the agent
as a dedicated unprivileged `ztop` user. The 0.1.0 package shipped the
per-user unit instead, created a `ztop` system user that nothing used,
and ran `systemctl --user` as root inside dpkg (which acts on root's
user manager, if any). It installed into `/usr/local`, which Debian
policy leaves to the local administrator. On purge it deleted
`/home/ztop` even when that user existed before the package. Its
Maintainer field and the man page carried an email address, which
AGENTS.md forbids, at a domain the project does not hold.

## Decision

- The package installs `/usr/bin/ztop`, a gzipped man page and the
  licenses under `/usr/share`, and `packaging/systemd/ztop-agent-system.service`
  as the system unit `ztop-agent.service`.
- The unit runs as `User=ztop` with `StateDirectory=ztop` (0700) and
  `XDG_CONFIG_HOME=/var/lib`, so the agent's state directory is
  `/var/lib/ztop`, plus the usual sandboxing (no capabilities,
  `NoNewPrivileges`, `ProtectSystem=strict`, `ProtectHome`, private
  `/tmp`, restricted address families, and so on). Devices stay
  visible for GPU metrics. `systemd-analyze security` rates it 3.3.
- `postinst` creates the system user with `adduser --system` only when
  it is missing, and records that in `/var/lib/ztop/.created-by-package`;
  then it enables and restarts the unit when systemd is running.
  `prerm` stops it and `postrm` reloads systemd.
- Purge deletes `/var/lib/ztop`, and the user only when the package
  created it. Nothing else is removed.
- The Maintainer field and the man page name the maintainers and point
  to the repository's issues, with no address.
- The per-user unit stays for installs without the package, and
  `check.sh` checks both units (no `--allow-control`, `User=ztop` in
  the system one, nothing under `/usr/local`). CI checks the user, the
  directory mode, that the service runs as `ztop`, and that a purge
  removes both.

## Consequences

- The service prints its pairing code to the journal, which only root
  and the `adm` and `systemd-journal` groups read
  (`sudo journalctl -u ztop-agent`).
- Upgrading from 0.1.0 removes the old files under `/usr/local` with the
  old package version; a user unit someone enabled by hand stays theirs.
- A Maintainer field without an address is accepted by `dpkg`, but not
  by the Debian archive; that matters only if the package is submitted
  there.
