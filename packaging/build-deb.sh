#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Builds zTop for Linux as a Debian package for this machine's architecture.
#
#   packaging/build-deb.sh [--build N] [--out DIR] [--skip-build]
#
# Writes DIR/ztop_<version>-<build>_<arch>.deb (default DIR: dist) and a .sha256 beside it.
# The version comes from the workspace Cargo.toml; the build number defaults to the commit
# count, so every build from main has a newer package version than the one before. With
# --skip-build it packages the release binary already in target/ (to make a second build
# number from the same binary, as the upgrade checks do).
#
# Build on the oldest distribution you want to support: the package needs that distribution's
# C library or newer. CI builds on Debian 12 (bookworm), which covers Debian 12 and 13,
# Raspberry Pi OS (bookworm and later), and Ubuntu 24.04 and later. CI builds each
# architecture natively, so dpkg-shlibdeps can read the binary.
#
# The package installs the apt repository it is published in (packaging/linux/ztop.sources)
# and the public key that signs it, so `apt upgrade` installs later versions.
set -euo pipefail
cd "$(dirname "$0")/.."

usage() { sed -n '4,20p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }
BUILD=""; OUT="dist"; SKIP_BUILD=0
while [ $# -gt 0 ]; do
  case "$1" in
    --build) BUILD="$2"; shift 2 ;;
    --out) OUT="$2"; shift 2 ;;
    --skip-build) SKIP_BUILD=1; shift ;;
    -h|--help) usage ;;
    *) echo "Unknown option $1" >&2; usage ;;
  esac
done
[ "$(uname)" = "Linux" ] || { echo "build-deb.sh runs on Linux." >&2; exit 1; }
for tool in dpkg-deb dpkg-shlibdeps gpg; do
  command -v "$tool" >/dev/null || { echo "$tool is missing: apt install dpkg-dev gnupg" >&2; exit 1; }
done

VERSION="$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml)"
[ -n "$VERSION" ] || { echo "No version in Cargo.toml's [workspace.package]" >&2; exit 1; }
if [ -z "$BUILD" ]; then
  BUILD="$(git rev-list --count HEAD 2>/dev/null)" || { echo "Not a git checkout; give --build N" >&2; exit 1; }
fi
case "$BUILD" in ''|*[!0-9]*) echo "--build must be a number" >&2; exit 2 ;; esac
ARCH="$(dpkg --print-architecture)"
PACKAGE_VERSION="$VERSION-$BUILD"

if [ "$SKIP_BUILD" -eq 0 ]; then
  cargo build --release --locked -p ztop
fi
BIN="target/release/ztop"
[ -x "$BIN" ] || { echo "No release ztop at $BIN" >&2; exit 1; }

WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
ROOT="$WORK/root"
L=packaging/linux
install -d "$ROOT/DEBIAN" "$ROOT/usr/bin" "$ROOT/usr/lib/systemd/user" "$ROOT/usr/share/man/man1" \
  "$ROOT/usr/share/doc/ztop" "$ROOT/usr/share/keyrings" "$ROOT/etc/apt/sources.list.d"
install -m 755 "$BIN" "$ROOT/usr/bin/ztop"
strip --strip-unneeded "$ROOT/usr/bin/ztop"
# The shipped unit names /usr/local/bin for people who install the binary by hand; the
# package puts it in /usr/bin.
sed 's|/usr/local/bin/ztop|/usr/bin/ztop|g' packaging/systemd/ztop-agent.service > "$ROOT/usr/lib/systemd/user/ztop-agent.service"
chmod 644 "$ROOT/usr/lib/systemd/user/ztop-agent.service"
grep -q '^ExecStart=/usr/bin/ztop ' "$ROOT/usr/lib/systemd/user/ztop-agent.service" \
  || { echo "The packaged unit doesn't start /usr/bin/ztop" >&2; exit 1; }
gzip -9n < packaging/man/ztop.1 > "$ROOT/usr/share/man/man1/ztop.1.gz"
chmod 644 "$ROOT/usr/share/man/man1/ztop.1.gz"
install -m 644 "$L/copyright" "$ROOT/usr/share/doc/ztop/copyright"
install -m 755 "$L/debian/postinst" "$ROOT/DEBIAN/postinst"

KEYRING="$L/ztop-archive-keyring.asc"
[ -f "$KEYRING" ] || { echo "No $KEYRING: the public key that signs zTop's apt repository" >&2; exit 1; }
gpg --dearmor < "$KEYRING" > "$ROOT/usr/share/keyrings/ztop-archive-keyring.gpg"
chmod 644 "$ROOT/usr/share/keyrings/ztop-archive-keyring.gpg"
install -m 644 "$L/ztop.sources" "$ROOT/etc/apt/sources.list.d/ztop.sources"
printf '%s\n' /etc/apt/sources.list.d/ztop.sources > "$ROOT/DEBIAN/conffiles"

# The shared libraries ztop links, as package names and versions.
mkdir -p "$WORK/shlibs/debian"
printf 'Source: ztop\n\nPackage: ztop\nArchitecture: any\n' > "$WORK/shlibs/debian/control"
SHLIBS="$(cd "$WORK/shlibs" && dpkg-shlibdeps -O "$ROOT/usr/bin/ztop" 2>/dev/null | sed -n 's/^shlibs:Depends=//p')"
[ -n "$SHLIBS" ] || { echo "dpkg-shlibdeps found no libraries for ztop" >&2; exit 1; }

SIZE="$(du -sk --exclude=DEBIAN "$ROOT" | cut -f1)"
cat > "$ROOT/DEBIAN/control" <<CONTROL
Package: ztop
Version: $PACKAGE_VERSION
Architecture: $ARCH
Maintainer: Bristlecone IT Services
Installed-Size: $SIZE
Depends: $SHLIBS
Section: utils
Priority: optional
Homepage: https://github.com/BITS-Freeware/zTop
Description: resource monitor for Macs and Linux machines, alone or as a fleet
 ztop is an interactive terminal monitor for processes, CPU, memory, disks,
 network, temperatures and GPUs. Run "ztop agent" and it serves the same
 metrics over pinned TLS with pairing, for the zTop app, a hub or another
 ztop; run "ztop hub" and one machine collects from every agent.
CONTROL

mkdir -p "$OUT"
DEB="$OUT/ztop_${PACKAGE_VERSION}_${ARCH}.deb"
dpkg-deb --root-owner-group -Zxz --build "$ROOT" "$DEB" >/dev/null
(cd "$OUT" && sha256sum "$(basename "$DEB")" > "$(basename "$DEB").sha256")
echo "$DEB"
