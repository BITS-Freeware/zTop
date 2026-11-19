#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# The Linux app's recorded check: two TLS agents on loopback (fake
# names host-a and host-b, each with its own HOME), the app pairing
# both through its seed hook with tokens in an unlocked gnome-keyring,
# then the app's own render of the window. Passes only when the app
# reports both hosts live and the tokens in the keyring.
#
#   desktop/scripts/recorded-run.sh <out.png> [kill-b-after-seconds]
#
# With a second argument, host-b's agent is stopped that many seconds
# after the app pairs with it, to record the stale or unreachable card instead; the check then
# expects host-b in the state named by EXPECT_B (default stale).
# With DETAIL=host-a the app opens that host's detail page instead and
# the check also expects the CPU, memory and load sections and the
# per-core bars; SNAPSHOT_AFTER (default 8) sets how long it runs, and
# SIZE (default 1000x1500) the window, so the page fits one picture.
# ALERT (a ZTOP_DESKTOP_ALERT rule) and OPEN (alerts or add) pass
# through to the app; with EXPECT_ALERT the check also expects that
# rule to have fired for host-b.
# SEARCH prefills the process search and SCROLL=processes scrolls the
# page to the process table.
# Needs xvfb-run, dbus-run-session and gnome-keyring-daemon; the app
# ztop and the app built (cargo build at the root and in desktop/).
set -eu
cd "$(dirname "$0")/../.."
OUT="$1"
KILL_B="${2:-}"
EXPECT_B="${EXPECT_B:-$([ -n "$KILL_B" ] && echo stale || echo live)}"
BIN="${ZTOP_BIN:-$PWD/target/debug/ztop}"
APP="${ZTOP_DESKTOP_BIN:-$PWD/desktop/target/debug/ztop-desktop}"
WORK="$(mktemp -d)"
trap 'kill $(cat "$WORK"/*.pid 2>/dev/null) 2>/dev/null || true; rm -rf "$WORK"' EXIT

start_agent() { # name port
    mkdir -p "$WORK/$1"
    env -i HOME="$WORK/$1" "$BIN" agent --tls --pair --no-advertise --port "$2" >"$WORK/$1.log" 2>&1 &
    echo $! >"$WORK/$1.pid"
}
seed_entry() { # name port
    i=0
    while ! grep -q "TLS fingerprint" "$WORK/$1.log" && [ $i -lt 100 ]; do sleep 0.1; i=$((i + 1)); done
    pin=$(awk '/TLS fingerprint/ {print $3}' "$WORK/$1.log")
    code=$(awk '/Pairing code/ {print $3}' "$WORK/$1.log")
    printf '%s|127.0.0.1|%s|%s|%s' "$1" "$2" "$pin" "$code"
}

PORT_A=$((20000 + $$ % 20000))
PORT_B=$((PORT_A + 1))
start_agent host-a "$PORT_A"
start_agent host-b "$PORT_B"
SEED="$(seed_entry host-a "$PORT_A");$(seed_entry host-b "$PORT_B")"

if [ -n "$KILL_B" ]; then
    # Count from host-b's pairing, not from now: the app pairs after its
    # own start-up, which can take longer than KILL_B on a slow machine,
    # and a host stopped before it pairs never becomes a card. The agent
    # writes tokens.json when a client pairs.
    (i=0
     while [ ! -s "$WORK/host-b/.config/ztop/tokens.json" ] && [ $i -lt 600 ]; do sleep 0.1; i=$((i + 1)); done
     sleep "$KILL_B"; kill "$(cat "$WORK/host-b.pid")") &
    # Five more seconds for the app's start-up and pairing, which the
    # stop now waits for.
    AFTER=$((5 + KILL_B + ${STALE_FOR:-4}))
else
    AFTER="${SNAPSHOT_AFTER:-8}"
fi
DETAIL_ENV=""
[ -n "${DETAIL:-}" ] && DETAIL_ENV="ZTOP_DESKTOP_DETAIL=$DETAIL ZTOP_DESKTOP_SIZE=${SIZE:-1000x1500} ZTOP_DESKTOP_SEARCH=${SEARCH:-} ZTOP_DESKTOP_SCROLL=${SCROLL:-}"

mkdir -p "$WORK/home/.cache"
cat >"$WORK/session.sh" <<INNER
eval \$(printf test | gnome-keyring-daemon --unlock --components=secrets)
ZTOP_DESKTOP_HOME="$WORK/config" ZTOP_DESKTOP_SEED="$SEED" \
ZTOP_DESKTOP_SNAPSHOT="$OUT" ZTOP_DESKTOP_SNAPSHOT_AFTER=$AFTER GTK_A11Y=none $DETAIL_ENV \
ZTOP_DESKTOP_ALERT="${ALERT:-}" ZTOP_DESKTOP_OPEN="${OPEN:-}" \
    xvfb-run -a -s "-screen 0 1600x2600x24" "$APP"
INNER
HOME="$WORK/home" dbus-run-session -- sh "$WORK/session.sh" 2>&1 | grep '^ztop-desktop:' | tee "$WORK/report"

grep -q "^ztop-desktop: wrote " "$WORK/report"
grep -q "^ztop-desktop: tokens in the desktop keyring$" "$WORK/report"
grep -q "^ztop-desktop: host host-a live$" "$WORK/report"
grep -q "^ztop-desktop: host host-b $EXPECT_B$" "$WORK/report"
if [ -n "${EXPECT_ALERT:-}" ]; then
    grep -q "^ztop-desktop: alert fired $EXPECT_ALERT on host-b$" "$WORK/report"
fi
if [ -n "${DETAIL:-}" ]; then
    for section in CPU Memory "Load average" "per-core bars" Processes; do
        grep -q "^ztop-desktop: detail shows $section$" "$WORK/report"
    done
fi
echo "recorded-run: ok ($OUT)"
