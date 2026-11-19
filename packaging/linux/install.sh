#!/bin/sh
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Installs the latest ztop on a Debian, Ubuntu, or Raspberry Pi OS computer.
#
#   curl -fsSL https://downloads.bristleconeit.com/ztop/install.sh | sh
#
# It downloads the package for this computer (amd64 or arm64), checks its SHA-256, and installs
# it with apt, which also adds zTop's signed apt repository, so `sudo apt upgrade` installs new
# versions from then on. Running it again installs the latest version.
#
# Published at downloads.bristleconeit.com/ztop/install.sh by the Publish Linux Packages
# workflow. ZTOP_DOWNLOADS points it at another copy of the downloads site, for testing.

# Everything is in a function that runs on the last line, so a download cut short runs nothing,
# and nothing in it can read the rest of the script from the pipe.
main() {
  set -eu
  base="${ZTOP_DOWNLOADS:-https://downloads.bristleconeit.com/ztop}"

  say() { printf '%s\n' "$*"; }
  die() { printf 'ztop: %s\n' "$*" >&2; exit 1; }

  [ "$(uname -s)" = Linux ] || die "this installer is for Linux. On a Mac, install ztop with Homebrew (see https://github.com/BITS-Freeware/zTop)"
  command -v dpkg >/dev/null 2>&1 && command -v apt-get >/dev/null 2>&1 \
    || die "this needs Debian, Ubuntu, Raspberry Pi OS, or another system that installs packages with apt"

  arch="$(dpkg --print-architecture)"
  case "$arch" in
    amd64|arm64) ;;
    armhf|armel|i386) die "ztop needs a 64-bit system (amd64 or arm64), and this one is $arch. On a Raspberry Pi, use the 64-bit Raspberry Pi OS." ;;
    *) die "ztop isn't built for $arch yet; it runs on amd64 and arm64" ;;
  esac

  if [ "$(id -u)" -eq 0 ]; then
    as_root() { "$@"; }
  elif command -v sudo >/dev/null 2>&1; then
    as_root() { sudo "$@"; }
    say "Installing needs administrator rights, so sudo may ask for your password."
  else
    die "run this as root, or install sudo"
  fi

  if command -v curl >/dev/null 2>&1; then
    fetch() { curl -fsSL --retry 3 -o "$2" "$1"; }
  elif command -v wget >/dev/null 2>&1; then
    fetch() { wget -q -O "$2" "$1"; }
  else
    die "this needs curl or wget to download ztop (sudo apt install curl)"
  fi
  command -v sha256sum >/dev/null 2>&1 || die "sha256sum is missing (it comes with coreutils)"

  # apt reads local packages as its own unprivileged user, so the folder must be readable.
  dir="$(mktemp -d)"
  trap 'rm -rf "$dir"' EXIT
  chmod 755 "$dir"
  deb="$dir/ztop_latest_$arch.deb"

  # The downloads site's cache can hold an older copy of a latest link for a while after a
  # release replaces it; a query string reads the current one, so the package and its checksum
  # always come from the same release.
  case "$base" in http*) q="?fresh=$(date +%s)" ;; *) q="" ;; esac
  say "Downloading ztop for $arch..."
  fetch "$base/linux/ztop_latest_$arch.deb.sha256$q" "$dir/sha256" || die "couldn't download $base/linux/ztop_latest_$arch.deb.sha256"
  fetch "$base/linux/ztop_latest_$arch.deb$q" "$deb" || die "couldn't download $base/linux/ztop_latest_$arch.deb"
  chmod 644 "$deb"
  want="$(cut -d' ' -f1 "$dir/sha256")"
  got="$(sha256sum "$deb" | cut -d' ' -f1)"
  [ -n "$want" ] && [ "$got" = "$want" ] \
    || die "the download doesn't match its checksum (got $got, expected $want), so it wasn't installed. Try again in a few minutes."
  version="$(dpkg-deb -f "$deb" Version)"

  # ztop needs only the C library, so there is no need to update the package lists first.
  say "Installing ztop $version..."
  as_root env DEBIAN_FRONTEND=noninteractive apt-get install -y "$deb" </dev/null \
    || die "apt couldn't install ztop $version; its messages are above"

  say ""
  say "ztop $version is installed, and sudo apt upgrade keeps it up to date."
  say "Run ztop for the monitor."
}

main "$@"
