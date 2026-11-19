#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
# Runs exactly what CI runs, on this machine.
#
# CI runs it on Linux and on macOS.
#
# Each stage's output is captured; on failure the tail is echoed and also
# emitted as GitHub `::error` annotations, which stay readable on public
# repos without authentication, unlike job logs.
set -eu
cd "$(dirname "$0")/.."

run() {
    stage="$1"
    shift
    log="${TMPDIR:-/tmp}/ztop-check-$stage.log"
    if "$@" >"$log" 2>&1; then
        echo "check: $stage ok"
    else
        code=$?
        echo "check: $stage FAILED (exit $code); last lines:"
        tail -n 30 "$log"
        # Annotations: one per line of the tail, CR-stripped and %-escaped.
        tail -n 12 "$log" | tr -d '\r' | sed 's/%/%25/g' | while IFS= read -r line; do
            printf '::error title=check.sh %s failed::%s\n' "$stage" "$line"
        done
        exit "$code"
    fi
}

run fmt cargo fmt --all -- --check
run clippy cargo clippy --workspace --all-targets -- -D warnings
run test cargo test --workspace
run smoke sh -c '
    set -e
    # Replay both committed fixtures through the production draw path.
    for arch in linux-x86_64 linux-aarch64; do
        frame=$(cargo run -q -p ztop -- replay "tests/fixtures/$arch")
        echo "$frame" | grep -q "$arch" || { echo "header missing $arch"; exit 1; }
        echo "$frame" | grep -Eq "[0-9]+ procs" || { echo "process count missing"; exit 1; }
        echo "$frame" | grep -q "proc-" || { echo "no process rows"; exit 1; }
        echo "$frame" | head -1
    done
'
run deny cargo deny check licenses bans sources
# The service files: the plist is linted where plutil exists (macOS);
# both are checked for the keys a service needs on either platform.
run packaging sh -c '
    set -e
    test -f packaging/launchd/com.ztop.agent.plist
    test -f packaging/systemd/ztop-agent.service
    grep -q "ProgramArguments" packaging/launchd/com.ztop.agent.plist
    grep -q "ExecStart" packaging/systemd/ztop-agent.service
    # The shipped services are read-only: no --allow-control argument.
    if grep -q "<string>--allow-control</string>" packaging/launchd/com.ztop.agent.plist; then
        echo "the launchd service must not allow control by default"
        exit 1
    fi
    if grep -q "^ExecStart=.*--allow-control" packaging/systemd/ztop-agent.service; then
        echo "the systemd service must not allow control by default"
        exit 1
    fi
    # The .deb system unit: read-only, and run as the ztop user.
    test -f packaging/systemd/ztop-agent-system.service
    if grep -q "^ExecStart=.*--allow-control" packaging/systemd/ztop-agent-system.service; then
        echo "the .deb system service must not allow control by default"
        exit 1
    fi
    grep -qx "User=ztop" packaging/systemd/ztop-agent-system.service
    # The package installs under /usr, never /usr/local.
    if grep -q "PKG_DIR/usr/local" packaging/build-deb.sh; then
        echo "the .deb must not install into /usr/local"
        exit 1
    fi
    # The Homebrew formula: syntax where ruby exists, and the shipped
    # service is read-only.
    if command -v ruby >/dev/null 2>&1; then
        ruby -c packaging/homebrew/ztop.rb
    fi
    if grep -q '"--allow-control"' packaging/homebrew/ztop.rb; then
        echo "the Homebrew service must not allow control by default"
        exit 1
    fi
    test -f packaging/man/ztop.1
    if command -v plutil >/dev/null 2>&1; then
        plutil -lint packaging/launchd/com.ztop.agent.plist
    fi
'

echo "check: all stages ok"
