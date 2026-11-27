#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
# Builds zTop's signed apt repository from Debian packages, ready to upload as-is under
# downloads.bristleconeit.com/ztop/apt/.
#
#   packaging/build-apt-repo.sh --out DIR --key-id FINGERPRINT [--gnupghome DIR] DEB...
#
# The signing key must already be in the GnuPG home (default: $GNUPGHOME, else ~/.gnupg). When
# APT_SIGNING_KEY_PASSPHRASE is set, it unlocks the key.
#
# Layout (one suite, "stable", one component, "main"):
#   pool/main/z/ztop/<package>.deb                 never replaced once published
#   dists/stable/main/binary-<arch>/Packages(.gz)
#   dists/stable/main/binary-<arch>/by-hash/SHA256/<hash>   the same indexes, by content
#   dists/stable/Release, Release.gpg, InRelease
# Only InRelease, Release, and Release.gpg change from one release to the next: apt fetches
# the indexes by hash (Acquire-By-Hash), so a cached old index can't be mistaken for the new.
# The indexes list the packages given here, which should be the newest build for each
# architecture. DIR starts empty and holds only those packages, so upload it by adding files,
# never by a sync that deletes: older packages then stay downloadable in the pool.
set -euo pipefail

usage() { sed -n '3,20p' "$0" | sed 's/^# \{0,1\}//'; exit 2; }
OUT=""; KEY=""; DEBS=()
while [ $# -gt 0 ]; do
  case "$1" in
    --out) OUT="$2"; shift 2 ;;
    --key-id) KEY="$2"; shift 2 ;;
    --gnupghome) export GNUPGHOME="$2"; shift 2 ;;
    -h|--help) usage ;;
    -*) echo "Unknown option $1" >&2; usage ;;
    *) DEBS+=("$1"); shift ;;
  esac
done
[ -n "$OUT" ] && [ -n "$KEY" ] && [ "${#DEBS[@]}" -gt 0 ] || usage
command -v apt-ftparchive >/dev/null || { echo "apt-ftparchive is missing: apt install apt-utils" >&2; exit 1; }

rm -rf "$OUT"
POOL="$OUT/pool/main/z/ztop"
mkdir -p "$POOL"
ARCHES=()
for deb in "${DEBS[@]}"; do
  [ "$(dpkg-deb -f "$deb" Package)" = "ztop" ] || { echo "$deb isn't the ztop package" >&2; exit 1; }
  arch="$(dpkg-deb -f "$deb" Architecture)"
  case " ${ARCHES[*]-} " in *" $arch "*) echo "Two packages for $arch" >&2; exit 1 ;; esac
  ARCHES+=("$arch")
  cp "$deb" "$POOL/"
done

cd "$OUT"
for arch in "${ARCHES[@]}"; do
  dir="dists/stable/main/binary-$arch"
  mkdir -p "$dir"
  apt-ftparchive --arch "$arch" packages pool > "$dir/Packages"
  gzip -9n < "$dir/Packages" > "$dir/Packages.gz"
  for index in Packages Packages.gz; do
    hash="$(sha256sum "$dir/$index" | cut -d' ' -f1)"
    mkdir -p "$dir/by-hash/SHA256"
    cp "$dir/$index" "$dir/by-hash/SHA256/$hash"
  done
done

apt-ftparchive \
  -o APT::FTPArchive::Release::Origin="Bristlecone IT Services" \
  -o APT::FTPArchive::Release::Label="zTop" \
  -o APT::FTPArchive::Release::Suite="stable" \
  -o APT::FTPArchive::Release::Codename="stable" \
  -o APT::FTPArchive::Release::Architectures="${ARCHES[*]}" \
  -o APT::FTPArchive::Release::Components="main" \
  -o APT::FTPArchive::Release::Description="zTop for Linux" \
  -o APT::FTPArchive::Release::Acquire-By-Hash="yes" \
  release dists/stable | grep -v '/by-hash/' > Release.tmp
# apt-ftparchive versions differ on whether they write Acquire-By-Hash; add it when missing.
grep -q '^Acquire-By-Hash:' Release.tmp || sed -i '/^Date:/a Acquire-By-Hash: yes' Release.tmp
mv Release.tmp dists/stable/Release

sign() {
  if [ -n "${APT_SIGNING_KEY_PASSPHRASE:-}" ]; then
    gpg --batch --yes --pinentry-mode loopback --passphrase-fd 3 --local-user "$KEY" "$@" 3<<<"$APT_SIGNING_KEY_PASSPHRASE"
  else
    gpg --batch --yes --local-user "$KEY" "$@"
  fi
}
sign --digest-algo SHA512 --clearsign -o dists/stable/InRelease dists/stable/Release
sign --digest-algo SHA512 --armor --detach-sign -o dists/stable/Release.gpg dists/stable/Release
echo "Built and signed $(pwd)"
