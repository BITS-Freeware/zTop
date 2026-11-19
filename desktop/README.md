# zTop for Linux

A GTK4 and libadwaita desktop app that watches the fleet: add an agent
or hub by address, fingerprint and pairing code, and see every host's
state, CPU, memory and uptime at a glance. Decision 0065 explains the
design.

## Build and run

Needs Rust stable and, on Ubuntu 24.04 or Debian 13:

```sh
sudo apt install libgtk-4-dev libadwaita-1-dev libsecret-1-dev
cargo run --manifest-path desktop/Cargo.toml
```

Hosts are saved in `~/.config/ztop-desktop/hosts.json` (or
`$XDG_CONFIG_HOME/ztop-desktop`); pairing tokens go to the desktop
keyring, or to 0600 files beside `hosts.json` when no keyring runs.

## Check

`desktop/scripts/check.sh` runs what CI's Desktop job runs. It also needs
`libsecret-1-dev`, `cargo-deny`, `xvfb`, `xauth`, `dbus-x11`, `gnome-keyring` and `libsecret-tools`:
the tests start a throwaway keyring in their own D-Bus session, and the
recorded run (`desktop/scripts/recorded-run.sh`) draws the window under
Xvfb with two paired agents, once on the fleet and once (`DETAIL=host-a`)
on a host's detail page.

## Layout

- `crates/desktop-core`: saved hosts, token store, live state, card
  figures, a host's recent history and the series its charts draw, the
  process table's search, sort and signal menu, and agent calls, through this repository's `ztop-core` client.
- `crates/desktop`: the window, the dialogs, the detail page and the
  keyring store.
