#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Checks an Arch package the way people get it: builds it from this checkout with
# makepkg-local.sh (tests included), lints the PKGBUILD and the package with namcap, installs it
# with pacman, checks what it put where, pairs a client with the installed agent, and removes
# it again. CI's Arch job runs it in an archlinux container.
#
# Run as root, on a disposable Arch machine or container, from a checkout:
#
#   packaging/arch/check-arch.sh ztop-monitor|ztop-desktop
#
# For ztop-desktop, desktop/scripts/check.sh is what runs the app; this checks the package.
# makepkg refuses root, so the build runs as a throwaway user, "builder".
set -euo pipefail
cd "$(dirname "$0")/../.."
PKG="${1:-ztop-monitor}"
[ "$(id -u)" -eq 0 ] || { echo "Run it as root." >&2; exit 1; }
fail() { echo "::error::$*"; exit 1; }
step() { echo "== $*"; }
pacman -Qq "$PKG" >/dev/null 2>&1 && fail "$PKG is already installed; run this on a clean machine"

step "Build $PKG from this checkout as an unprivileged user"
id builder >/dev/null 2>&1 || useradd -m builder
SRC="$(mktemp -d)"; AGENT=""
trap '[ -z "$AGENT" ] || kill "$AGENT" 2>/dev/null || true; rm -rf "$SRC"' EXIT
git config --global --add safe.directory "$PWD"
git clone -q "$PWD" "$SRC/zTop"
chown -R builder: "$SRC"
su builder -c "cd '$SRC/zTop' && packaging/arch/makepkg-local.sh '$PKG'"
FILE="$(ls "$SRC"/zTop/dist/arch/"$PKG"-[0-9]*.pkg.tar.* 2>/dev/null | grep -v '\.sig$' | head -1)"
[ -f "$FILE" ] || fail "makepkg made no $PKG package"

step "namcap finds no errors in the PKGBUILD or the package"
namcap "packaging/arch/$PKG/PKGBUILD" | tee "$SRC/namcap.txt"
namcap "$FILE" | tee -a "$SRC/namcap.txt"
if grep -E '^[^ ]+ E: ' "$SRC/namcap.txt"; then fail "namcap reported errors"; fi

step "pacman installs it"
pacman -U --noconfirm "$FILE"

case "$PKG" in
ztop-monitor)
  step "The package installs into /usr"
  # The package's own file list: Docker's archlinux image tells pacman not to extract man pages.
  FILES="$(pacman -Qlq "$PKG")"
  for f in /usr/bin/ztop /usr/lib/systemd/user/ztop-agent.service /usr/share/man/man1/ztop.1.gz \
           /usr/lib/systemd/system/ztop-agent.service /usr/lib/sysusers.d/ztop.conf \
           /etc/default/ztop-agent /usr/lib/ztop/pair-system-agent /usr/share/doc/ztop/README.server \
           /usr/share/licenses/ztop-monitor/LICENSE-MIT /usr/share/licenses/ztop-monitor/LICENSE-APACHE \
           /usr/share/doc/ztop/arch.md; do
    grep -qx "$f" <<<"$FILES" || fail "$PKG has no $f"
  done
  grep -q '^ExecStart=/usr/bin/ztop agent --tls --pair$' /usr/lib/systemd/user/ztop-agent.service \
    || fail "the packaged unit doesn't start /usr/bin/ztop"
  grep -q '^ExecStart=/usr/bin/ztop agent --tls --no-advertise ' /usr/lib/systemd/system/ztop-agent.service \
    || fail "the system unit doesn't start /usr/bin/ztop"
  # systemd treats lines starting with # or ; as comments.
  ! grep -hv '^[[:space:]]*[#;]' /usr/lib/systemd/system/ztop-agent.service /etc/default/ztop-agent \
      | grep -Eq -- '--pair|--allow-control|--allow-manage' \
    || fail "the system agent must never pair or grant control or management"
  getent passwd ztop >/dev/null || fail "pacman's sysusers hook made no ztop user"
  ztop --version

  step "pair-system-agent pairs a read-only client with the system agent's state"
  PAIRED="$(/usr/lib/ztop/pair-system-agent --port $((21000 + $$ % 20000)))"
  grep -Eq '^\{"pin":"SHA256:[0-9a-f]+","token":"[0-9a-f]+"\}$' <<<"$PAIRED" || fail "pair-system-agent printed $PAIRED"
  [ "$(stat -c %U:%a /var/lib/ztop)" = "ztop:700" ] || fail "/var/lib/ztop is not ztop's alone"

  step "A client pairs with the installed agent"
  HOME_DIR="$SRC/agent-home"; mkdir -p "$HOME_DIR"
  PORT=$((20000 + $$ % 20000))
  ! (exec 3<>"/dev/tcp/127.0.0.1/$PORT") 2>/dev/null || fail "port $PORT is already taken"
  HOME="$HOME_DIR" ztop agent --tls --pair --no-advertise --port "$PORT" >"$HOME_DIR/agent.log" 2>&1 &
  AGENT=$!
  for _ in $(seq 50); do grep -q '^TLS fingerprint:' "$HOME_DIR/agent.log" && break; sleep 0.2; done
  code="$(sed -n 's/^Pairing code: \([A-Za-z0-9]*\).*/\1/p' "$HOME_DIR/agent.log" | head -1)"
  pin="$(sed -n 's/^TLS fingerprint: \(SHA256:[0-9a-f]*\)$/\1/p' "$HOME_DIR/agent.log" | head -1)"
  [ -n "$code" ] && [ -n "$pin" ] || { cat "$HOME_DIR/agent.log" >&2; fail "the agent printed no pairing code and fingerprint"; }
  HOME="$HOME_DIR" ztop connect --tls --pin "$pin" --code "$code" --once "127.0.0.1:$PORT"
  kill "$AGENT"; wait "$AGENT" 2>/dev/null || true; AGENT=""
  ;;
ztop-desktop)
  step "The package installs into /usr with a launcher entry"
  FILES="$(pacman -Qlq "$PKG")"
  for f in /usr/bin/ztop-desktop /usr/share/applications/dev.ztop.Desktop.desktop \
           /usr/share/licenses/ztop-desktop/LICENSE-MIT /usr/share/licenses/ztop-desktop/LICENSE-APACHE; do
    grep -qx "$f" <<<"$FILES" || fail "$PKG has no $f"
  done
  if command -v desktop-file-validate >/dev/null; then
    desktop-file-validate /usr/share/applications/dev.ztop.Desktop.desktop
  fi
  ;;
esac

step "pacman removes it"
pacman -R --noconfirm "$PKG"
! pacman -Qq "$PKG" >/dev/null 2>&1 || fail "$PKG is still installed"
echo "check-arch: $PKG ok"
