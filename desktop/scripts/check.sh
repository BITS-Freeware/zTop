#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Everything CI's Desktop job runs for the Linux desktop app, on any Linux box
# with the packages in desktop/README.md: the ztop binary (the
# live tests and the recorded run spawn it), format and lint, the
# tests (live agents, and the Secret Service store against a throwaway
# gnome-keyring in its own D-Bus session), then the recorded run with
# two paired agents, on the fleet and on a host's detail page.
set -eu
cd "$(dirname "$0")/../.."
ROOT="$PWD"

run() {
    stage="$1"
    shift
    log="${TMPDIR:-/tmp}/ztop-linux-check-$stage.log"
    if "$@" >"$log" 2>&1; then
        echo "check: $stage ok"
    else
        code=$?
        echo "check: $stage FAILED (exit $code); last lines:"
        tail -n 40 "$log"
        exit "$code"
    fi
}

# The keyring session gets its own HOME; cargo and rustup must not.
export CARGO_HOME="${CARGO_HOME:-$HOME/.cargo}"
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/.rustup}"
export ZTOP_BIN="$ROOT/target/debug/ztop"
export ZTOP_REQUIRE_LIVE=1

run ztop cargo build -p ztop
run fmt cargo fmt --manifest-path desktop/Cargo.toml --all --check
run clippy cargo clippy --manifest-path desktop/Cargo.toml --all-targets -- -D warnings
run build cargo build --manifest-path desktop/Cargo.toml
run deny cargo deny --manifest-path desktop/Cargo.toml check

KEYHOME="$(mktemp -d)"
trap 'rm -rf "$KEYHOME"' EXIT
mkdir -p "$KEYHOME/.cache"
run test env HOME="$KEYHOME" dbus-run-session -- sh -c '
    eval "$(printf test | gnome-keyring-daemon --unlock --components=secrets)"
    ZTOP_TEST_KEYRING=1 cargo test --manifest-path desktop/Cargo.toml'
run recorded-run desktop/scripts/recorded-run.sh "$KEYHOME/overview.png"
run recorded-detail env DETAIL=host-a SNAPSHOT_AFTER=12 desktop/scripts/recorded-run.sh "$KEYHOME/detail.png"

echo "check: all linux stages ok"
