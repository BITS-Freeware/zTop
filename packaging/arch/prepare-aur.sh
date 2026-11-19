#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Gets a package ready to push to the AUR for a release that is already tagged on GitHub.
#
#   packaging/arch/prepare-aur.sh ztop-monitor|ztop-desktop <aur-checkout>
#
# <aur-checkout> is a clone of ssh://aur@aur.archlinux.org/<name>.git. The script copies the
# PKGBUILD and its .install file there, replaces the SKIP checksum with the release tarball's
# real SHA-256 (updpkgsums downloads it), and writes .SRCINFO. It commits nothing and pushes
# nothing: review the diff, then commit and push from the AUR checkout yourself.
set -euo pipefail
cd "$(dirname "$0")/../.."
PKG="${1:-}"; DEST="${2:-}"
[ -f "packaging/arch/$PKG/PKGBUILD" ] && [ -d "$DEST/.git" ] \
  || { echo "usage: $0 ztop-monitor|ztop-desktop <aur-checkout>" >&2; exit 2; }
command -v updpkgsums >/dev/null || { echo "updpkgsums is missing: pacman -S pacman-contrib" >&2; exit 1; }
cp "packaging/arch/$PKG/"* "$DEST/"
(cd "$DEST" && updpkgsums && makepkg --printsrcinfo > .SRCINFO && git status --short)
