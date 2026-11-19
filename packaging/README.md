# zTop services

Every service file runs the agent as a **read-only** long-lived service:
TLS with pairing, no `--allow-control`, so nothing paired through them
can signal or renice. To allow control, add `--allow-control` to
`ProgramArguments` (launchd) or `ExecStart` (systemd) and restart the
service; tokens paired before the change keep their read-only rights —
pair again afterwards to hold the `control` right (decision 0042).

The per-user files point at `/usr/local/bin/ztop`, where a binary you
installed by hand usually lives; change it to wherever yours is.

## Debian and Ubuntu (the .deb, system-wide)

The package installs `/usr/bin/ztop` and
`systemd/ztop-agent-system.service` as the system unit
`ztop-agent.service`. It creates the unprivileged `ztop` system user,
runs the agent as that user with its state in `/var/lib/ztop` (mode
0700) and a sandboxed unit, and enables and starts it. Read the pairing
code and fingerprint with `sudo journalctl -u ztop-agent`; restart the
service for a fresh code. Purging the package deletes `/var/lib/ztop`,
and the `ztop` user only if the package created it.

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
