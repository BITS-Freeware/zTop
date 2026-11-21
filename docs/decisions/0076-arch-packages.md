# 0076 — Arch Linux packages

Date: 2026-11-19
Status: accepted

## Context

zTop shipped for Linux only as a `.deb` for Debian, Ubuntu and
Raspberry Pi OS. People on Arch Linux and Arch-based systems such as
Omarchy (Arch with Hyprland on Wayland) had to build from source by
hand and install the systemd unit themselves.

The engine reads only kernel interfaces (`/proc`, `/sys`) and links
only glibc and libgcc, so nothing in it is specific to Debian. Its
tests pass in an `archlinux` container, and the agent pairs there.

The AUR already has a package named `ztop`: an unrelated ZFS I/O
viewer that also installs `/usr/bin/ztop`.

## Decision

- `packaging/arch/ztop-monitor/PKGBUILD` builds the `ztop` command
  from a release tag's GitHub tarball, runs the workspace tests in
  `check()`, and installs the binary, the agent's systemd user unit
  (pointed at `/usr/bin/ztop`, as in the `.deb`), the man page and the
  licenses. The package is named **ztop-monitor**, `provides` and
  `conflicts` with `ztop`, and keeps the `ztop` command. The desktop
  app's package will be **ztop-desktop**.
- It also ships the system agent for servers exactly as the `.deb` does
  (decision 0066): the system unit, the `ztop` user through
  `sysusers.d` (pacman's hook creates it), `/etc/default/ztop-agent` as
  a backup file, `pair-system-agent` and `README.server`, all under the
  same paths, with docs in `/usr/share/doc/ztop`. Removing the package
  stops and disables the system agent; its state in `/var/lib/ztop`
  stays, as Arch packages leave state behind.
- Like the `.deb`, it installs the agent as a user service that stays
  off until someone turns it on, listening on loopback. The Arch notes
  (`packaging/arch/README.md`) say how to serve the network with a
  drop-in and, on Omarchy, which ufw ports to open; the package does
  not change the firewall or the unit's bind address.
- `packaging/arch/makepkg-local.sh` builds the package from a checkout
  (`git archive HEAD` as the tarball the PKGBUILD names), so a branch
  can be tested before it is tagged.
- `packaging/arch/check-arch.sh` builds, lints with namcap, installs,
  pairs with and removes the package. The Packaging workflow runs it in
  an `archlinux` container on a GitHub-hosted runner, x86_64 only
  (Docker Hub's image has no aarch64 build). No fleet runner or secret
  is involved.
- Publishing to the AUR is a person's step:
  `packaging/arch/prepare-aur.sh` fills in the release tarball's
  checksum and writes `.SRCINFO` in an AUR checkout, and commits and
  pushes nothing.

## Consequences

- Arch users install with makepkg today and from the AUR once a release
  is uploaded there; `pacman -Syu` with an AUR helper keeps it current.
- Each release needs `pkgver` bumped in the PKGBUILD and an AUR upload,
  besides the `.deb` and the Homebrew formula.
- aarch64 is declared but checked by nobody in CI.
