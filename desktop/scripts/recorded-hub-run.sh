#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# The Linux app's recorded check through a hub: three plain agents on
# loopback (fake names host-a, host-b and host-c, each with its own
# HOME), one TLS hub watching them, and the app pairing only with the
# hub through its seed hook. Passes only when the app reports the hub
# and all three machines live.
#
#   desktop/scripts/recorded-hub-run.sh <out.png>
#
# Needs what recorded-run.sh needs.
set -eu
cd "$(dirname "$0")/../.."
OUT="$1"
BIN="${ZTOP_BIN:-$PWD/target/debug/ztop}"
APP="${ZTOP_DESKTOP_BIN:-$PWD/desktop/target/debug/ztop-desktop}"
WORK="$(mktemp -d)"
trap 'kill $(cat "$WORK"/*.pid 2>/dev/null) 2>/dev/null || true; rm -rf "$WORK"' EXIT

BASE=$((22000 + $$ % 18000))
AGENTS=""
i=0
for name in host-a host-b host-c; do
    port=$((BASE + i))
    mkdir -p "$WORK/$name"
    env -i HOME="$WORK/$name" "$BIN" agent --port "$port" >"$WORK/$name.log" 2>&1 &
    echo $! >"$WORK/$name.pid"
    AGENTS="$AGENTS${AGENTS:+,}{\"name\":\"$name\",\"target\":\"127.0.0.1:$port\"}"
    i=$((i + 1))
done
printf '{"agents":[%s]}\n' "$AGENTS" >"$WORK/agents.json"

HUB_PORT=$((BASE + 3))
mkdir -p "$WORK/hub"
env -i HOME="$WORK/hub" "$BIN" hub --tls --pair --port "$HUB_PORT" --agents "$WORK/agents.json" >"$WORK/hub.log" 2>&1 &
echo $! >"$WORK/hub.pid"
i=0
while ! grep -q "Pairing code" "$WORK/hub.log" && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
pin=$(awk '/TLS fingerprint/ {print $3}' "$WORK/hub.log")
code=$(awk '/Pairing code/ {print $3}' "$WORK/hub.log")
SEED="hub-1|127.0.0.1|$HUB_PORT|$pin|$code|hub"

mkdir -p "$WORK/home/.cache"
cat >"$WORK/session.sh" <<INNER
eval \$(printf test | gnome-keyring-daemon --unlock --components=secrets)
ZTOP_DESKTOP_HOME="$WORK/config" ZTOP_DESKTOP_SEED="$SEED" \
ZTOP_DESKTOP_SNAPSHOT="$OUT" ZTOP_DESKTOP_SNAPSHOT_AFTER=${SNAPSHOT_AFTER:-10} GTK_A11Y=none \
    xvfb-run -a -s "-screen 0 1280x800x24" "$APP"
INNER
HOME="$WORK/home" dbus-run-session -- sh "$WORK/session.sh" 2>&1 | grep '^ztop-desktop:' | tee "$WORK/report"

grep -q "^ztop-desktop: wrote " "$WORK/report"
for host in hub-1 host-a host-b host-c; do
    grep -q "^ztop-desktop: host $host live$" "$WORK/report"
done
echo "recorded-hub-run: ok ($OUT)"
