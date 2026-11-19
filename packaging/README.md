# zTop services

Both service files run the agent as a **read-only** long-lived service:
TLS with pairing, no `--allow-control`, so nothing paired through them
can signal or renice. To allow control, add `--allow-control` to
`ProgramArguments` (launchd) or `ExecStart` (systemd) and restart the
service; tokens paired before the change keep their read-only rights —
pair again afterwards to hold the `control` right (decision 0042).

Both point at `/usr/local/bin/ztop`; change it to wherever your
package manager installed the binary. The `.deb` (below) ships its
own copy of the systemd unit, pointed at `/usr/bin/ztop`.

## macOS (launchd, per user)

    sed "s|/Users/YOU|$HOME|g" launchd/com.ztop.agent.plist \
        > ~/Library/LaunchAgents/com.ztop.agent.plist
    launchctl load ~/Library/LaunchAgents/com.ztop.agent.plist

The `sed` fills in your home directory: launchd needs absolute log
paths, and the log belongs in `~/Library/Logs`.

Stop: `launchctl unload ~/Library/LaunchAgents/com.ztop.agent.plist`.

## Linux (systemd, per user)

    mkdir -p ~/.config/systemd/user
    cp systemd/ztop-agent.service ~/.config/systemd/user/
    systemctl --user daemon-reload
    systemctl --user enable --now ztop-agent

Stop: `systemctl --user stop ztop-agent`.

The agent binds 127.0.0.1 only; TLS + pairing is what lets a client on
another machine talk to it. First start prints the pairing code and the
TLS fingerprint to the service's log (`~/Library/Logs/ztop-agent.log`
under launchd, `$(brew --prefix)/var/log/ztop-agent.log` under
`brew services`, `journalctl --user -u ztop-agent` under systemd).

The pairing code is a credential for its five minutes, so the agent
guards it (decision 0061): before printing it into a log file, the
agent narrows that file to its owner (mode 0600); if another user owns
the file, the code goes to `pairing-code` (mode 0600) in the agent's
state directory instead and the log only says where. Five wrong
guesses burn a code, and guesses are spaced a second apart.

Token management (`GET /v1/tokens`, `DELETE /v1/tokens/{id}`) needs a
token paired with `--allow-manage`; the shipped services do not pass
it, so no client paired through them can list or revoke tokens. To
manage tokens, stop the service and run `ztop agent --tls --pair
--allow-manage` once by hand to pair your own client.

## Debian and Ubuntu (apt)

    curl -fsSL https://downloads.bristleconeit.com/ztop/install.sh | sh

The installer ([linux/install.sh](linux/install.sh)) downloads the
package for this machine (amd64 or arm64), checks its SHA-256, and
installs it with apt. The package adds zTop's signed apt repository
(`/etc/apt/sources.list.d/ztop.sources`, signed by the key in
[linux/ztop-archive-keyring.asc](linux/ztop-archive-keyring.asc)), so
`sudo apt upgrade` installs later versions. It runs on Debian 12 and 13,
Raspberry Pi OS (64-bit, bookworm and later) and Ubuntu 24.04 and later.

The package installs `/usr/bin/ztop`, the man page, and the agent as a
user service and a system service (below), both off until you turn one
on. On a desktop:

    systemctl --user enable --now ztop-agent

### Servers (systemd, system-wide)

A server with nobody logged in uses the system service instead, also
installed by the package and off until you turn it on. It runs as the
unprivileged `ztop` user, keeps its certificate and tokens in
`/var/lib/ztop`, and never pairs, signals or renices (decision 0066):

    # 1. The address to serve: a private network's address on this machine.
    echo 'ZTOP_AGENT_ARGS="--bind 100.64.0.10"' | sudo tee /etc/default/ztop-agent
    # 2. Pair a client; prints {"pin":"SHA256:...","token":"..."}.
    sudo /usr/lib/ztop/pair-system-agent
    # 3. Start it, now and at boot.
    sudo systemctl enable --now ztop-agent

The pairing helper trades the code over loopback, so it never reaches
the journal or the network. Give the pin and token to the client, such
as one entry in a hub's agents file. The unit starts after
`tailscaled` when Tailscale is installed, and retries every five
seconds until the address it binds exists.
[linux/README.server](linux/README.server), installed as
`/usr/share/doc/ztop/README.server`, has the rest.

To add the repository by hand instead:

    curl -fsSL https://downloads.bristleconeit.com/ztop/apt/ztop-archive-keyring.asc \
        | sudo gpg --dearmor -o /usr/share/keyrings/ztop-archive-keyring.gpg
    sudo cp packaging/linux/ztop.sources /etc/apt/sources.list.d/
    sudo apt update && sudo apt install ztop

The repository is signed with the "Bristlecone IT Services apt signing"
key (Ed25519, fingerprint `95AC 000B 4F4D 70F2 4276  DA2A 768E E42A 6FCB
ED6D`, expires 2029-11-08).

### Building and releasing

- `packaging/build-deb.sh` builds `dist/ztop_<version>-<build>_<arch>.deb`
  for this machine's architecture, on Linux. The build number is the
  commit count.
- `packaging/check-deb.sh dist/ztop_*.deb` (as root, on a disposable
  machine) checks the package's contents, installs it with the one-line
  installer from a local copy of the downloads site, and upgrades it
  through a repository signed with a throwaway key. The Packaging
  workflow runs both on Debian 12 for amd64 and arm64 and keeps the
  packages as the `ztop-linux-amd64` and `ztop-linux-arm64` artifacts.
- **Publish Linux Packages** (`.github/workflows/publish-linux.yml`),
  run by hand from main with a Packaging run's number, signs the
  repository with `packaging/build-apt-repo.sh` and uploads it, the
  latest links and the installer. Its secrets live in the `apt-publish`
  environment, which only main can deploy to: `APT_SIGNING_KEY` (the
  signing subkey only), `APT_SIGNING_KEY_PASSPHRASE`, `R2_ACCESS_KEY_ID`,
  `R2_SECRET_ACCESS_KEY`, `R2_ENDPOINT` and `R2_BUCKET`. Try it first
  with `dry_run`.
