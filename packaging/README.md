# zTop services

Both service files run the agent as a **read-only** long-lived service:
TLS with pairing, no `--allow-control`, so nothing paired through them
can signal or renice. To allow control, add `--allow-control` to
`ProgramArguments` (launchd) or `ExecStart` (systemd) and restart the
service; tokens paired before the change keep their read-only rights —
pair again afterwards to hold the `control` right (decision 0042).

Both point at `/usr/local/bin/ztop`; change it to wherever your
package manager installed the binary.

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
