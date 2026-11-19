#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Builds an Arch package from this checkout instead of a release tag, so a branch can be
# installed and tested before it is released.
#
#   packaging/arch/makepkg-local.sh ztop-monitor|ztop-desktop [makepkg options]
#
# Packs the checkout's committed tree (git archive HEAD, so uncommitted changes are left out)
# as the release tarball the PKGBUILD names, then runs makepkg on a copy of the PKGBUILD with
# that tarball already in place. makepkg uses a source file it already has instead of
# downloading it. Pass makepkg's own options after the name, such as -si to install the
# package or --nocheck to skip the tests. Writes the package to dist/arch/. Run it as a normal
# user, never as root: makepkg refuses root.
set -euo pipefail
cd "$(dirname "$0")/../.."
PKG="${1:-}"; shift || true
[ -f "packaging/arch/$PKG/PKGBUILD" ] || { echo "usage: $0 ztop-monitor|ztop-desktop [makepkg options]" >&2; exit 2; }
command -v makepkg >/dev/null || { echo "makepkg is missing: this runs on Arch (pacman -S base-devel)" >&2; exit 1; }
[ "$(id -u)" -ne 0 ] || { echo "Run it as a normal user; makepkg refuses root." >&2; exit 1; }

PKGVER="$(sed -n 's/^pkgver=//p' "packaging/arch/$PKG/PKGBUILD")"
WORK="$(mktemp -d)"; trap 'rm -rf "$WORK"' EXIT
cp "packaging/arch/$PKG/"* "$WORK/"
git archive --format=tar.gz --prefix="zTop-$PKGVER/" -o "$WORK/zTop-$PKGVER.tar.gz" HEAD
mkdir -p dist/arch
OUT="$(realpath dist/arch)"
(cd "$WORK" && PKGDEST="$OUT" SRCDEST="$WORK" makepkg -f --skipchecksums "$@")
ls "$OUT"/"$PKG"-*.pkg.tar.* 
