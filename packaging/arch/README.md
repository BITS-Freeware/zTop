# zTop on Arch Linux

Two packages for Arch Linux and Arch-based systems such as Omarchy
(decision 0076):

- **ztop-monitor**: the `ztop` command, so the terminal monitor, `ztop
  agent` and `ztop hub`, with the agent's systemd user unit, the system
  agent for servers (as in the `.deb`; see
  `/usr/share/doc/ztop/README.server`) and the man page. The AUR's `ztop` is an unrelated ZFS I/O viewer that also
  installs `/usr/bin/ztop`, so this package has another name and
  conflicts with that one.
- **ztop-desktop**: zTop for Linux, the GTK4 desktop app in
  [desktop/](../../desktop).

Both build from source with makepkg and run on x86_64 and aarch64.

## Install from a checkout

Until the packages are on the AUR, build them from this repository:

    sudo pacman -S --needed base-devel git rust
    git clone https://github.com/BITS-Freeware/zTop && cd zTop
    packaging/arch/makepkg-local.sh ztop-monitor -si

`makepkg-local.sh` packs the checkout's last commit as the release
tarball the PKGBUILD expects and runs makepkg on it. Options after the
package name go to makepkg: `-si` installs the package and its
dependencies, `--nocheck` skips the tests. The package lands in
`dist/arch/`. Run it as yourself, not with sudo; makepkg asks for sudo
when it installs.

## Serve this machine to another one

The agent is a user service that stays off until you turn it on:

    systemctl --user enable --now ztop-agent
    journalctl --user -u ztop-agent    # the pairing code and TLS fingerprint

The shipped unit listens on 127.0.0.1 only, so on its own it serves this
machine and nothing else. To serve the network, override its command
line with a drop-in (`systemctl --user edit ztop-agent`):

    [Service]
    ExecStart=
    ExecStart=/usr/bin/ztop agent --tls --pair --bind 0.0.0.0

then `systemctl --user restart ztop-agent`. TLS and pairing guard every
connection either way; a client needs the pairing code, valid for five
minutes, and should check the fingerprint.

The agent keeps running only while you are logged in. To keep it
running after you log out, and from boot, run
`sudo loginctl enable-linger $USER` once.

### Firewall

Omarchy turns on ufw and denies incoming connections, so another
machine cannot reach the agent until you allow its port, and nothing
nearby sees it over Bonjour until you allow mDNS:

    sudo ufw allow 7100/tcp comment 'zTop agent'
    sudo ufw allow 5353/udp comment 'mDNS'

A hub listens on 7101 (`sudo ufw allow 7101/tcp`). Other Arch systems
have no firewall unless you set one up.

## Publish to the AUR

For a release that is tagged on GitHub, with an AUR account that has
your SSH key:

    sudo pacman -S --needed pacman-contrib
    git clone ssh://aur@aur.archlinux.org/ztop-monitor.git ~/aur/ztop-monitor
    packaging/arch/prepare-aur.sh ztop-monitor ~/aur/ztop-monitor

The script copies the PKGBUILD over, fills in the release tarball's
SHA-256 and writes `.SRCINFO`. It commits and pushes nothing: review
the diff, then commit and push from `~/aur/ztop-monitor`. For each new
release, set `pkgver` in the PKGBUILD here first and reset `pkgrel` to
1.

## Check

`packaging/arch/check-arch.sh ztop-monitor`, as root on a disposable
Arch machine or container, builds the package from the checkout with
its tests, lints it with namcap, installs it, pairs a client with the
installed agent and removes it. The Packaging workflow's Arch job runs
it in an `archlinux` container on a GitHub-hosted runner.
