#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Checks a ztop package the way people get it: its contents, the one-line installer
# (packaging/linux/install.sh) piped into sh against a local copy of the downloads site, and an
# upgrade through a signed apt repository holding the next build. apt must refuse a download
# that doesn't match its checksum and a repository whose index was changed after signing.
# Then purges ztop. Signs with a throwaway key made for the run, never the real one.
#
# Run as root, on a disposable machine or container, from a checkout where
# packaging/build-deb.sh has just built the package (it reuses that binary):
#
#   packaging/check-deb.sh dist/ztop_<version>-<build>_<arch>.deb
set -euo pipefail
DEB="$(realpath "$1")"
cd "$(dirname "$0")/.."
[ "$(id -u)" -eq 0 ] || { echo "Run it as root." >&2; exit 1; }
fail() { echo "::error::$*"; exit 1; }
step() { echo "== $*"; }
export DEBIAN_FRONTEND=noninteractive

ARCH="$(dpkg-deb -f "$DEB" Architecture)"
VERSION="$(dpkg-deb -f "$DEB" Version)"
BUILD="${VERSION##*-}"
NEXT=$((BUILD + 1))
WORK="$(mktemp -d)"
# apt reads local packages and file: repositories as its own unprivileged user, so neither can
# be in a private temporary folder.
SITE=/srv/ztop-site-check
REPO=/srv/ztop-apt-check
trap 'rm -rf "$WORK" "$SITE" "$REPO"' EXIT
dpkg -s ztop >/dev/null 2>&1 && fail "ztop is already installed; run this on a clean machine"

step "The package installs into /usr and carries its repository and key"
CONTENTS="$(dpkg-deb -c "$DEB")"
for f in ./usr/bin/ztop ./usr/lib/systemd/user/ztop-agent.service ./usr/share/man/man1/ztop.1.gz \
         ./usr/share/doc/ztop/copyright ./etc/apt/sources.list.d/ztop.sources \
         ./usr/share/keyrings/ztop-archive-keyring.gpg ./usr/lib/systemd/system/ztop-agent.service \
         ./usr/lib/sysusers.d/ztop.conf ./etc/default/ztop-agent ./usr/lib/ztop/pair-system-agent \
         ./usr/share/doc/ztop/README.server; do
  grep -q " $f\$" <<<"$CONTENTS" || fail "$DEB has no $f"
done
grep -q ' \./usr/local' <<<"$CONTENTS" && fail "$DEB installs into /usr/local, which belongs to the person, not to packages"
CONFFILES="$(dpkg-deb --ctrl-tarfile "$DEB" | tar -xO ./conffiles)"
grep -qx /etc/apt/sources.list.d/ztop.sources <<<"$CONFFILES" \
  || fail "the sources entry isn't a conffile, so a person's change to it would be overwritten"
grep -qx /etc/default/ztop-agent <<<"$CONFFILES" \
  || fail "the system agent's options file isn't a conffile, so an upgrade would reset its address"
mkdir -p "$WORK/extract"; dpkg-deb -x "$DEB" "$WORK/extract"
grep -q '^ExecStart=/usr/bin/ztop agent ' "$WORK/extract/usr/lib/systemd/user/ztop-agent.service" \
  || fail "the packaged agent unit doesn't start /usr/bin/ztop"
grep '^ExecStart=' "$WORK/extract/usr/lib/systemd/user/ztop-agent.service" | grep -q -- '--allow-control' \
  && fail "the packaged agent unit allows control; it must be read-only"
SYSTEM_UNIT="$WORK/extract/usr/lib/systemd/system/ztop-agent.service"
grep -qx 'User=ztop' "$SYSTEM_UNIT" || fail "the system agent unit doesn't run as the ztop user"
grep -q '^ExecStart=/usr/bin/ztop agent --tls ' "$SYSTEM_UNIT" || fail "the system agent unit doesn't serve TLS"
grep '^ExecStart=' "$SYSTEM_UNIT" | grep -qE -- '--allow-control|--allow-manage|--pair' \
  && fail "the system agent unit pairs or grants control or management; it must be read-only and never pair"
grep -qE '^ZTOP_AGENT_ARGS=.*--(allow-control|allow-manage|pair)' "$WORK/extract/etc/default/ztop-agent" \
  && fail "the shipped options file pairs or grants rights"
WANT="$(gpg --show-keys --with-colons packaging/linux/ztop-archive-keyring.asc | awk -F: '/^fpr/{print $10; exit}')"
GOT="$(gpg --show-keys --with-colons "$WORK/extract/usr/share/keyrings/ztop-archive-keyring.gpg" | awk -F: '/^fpr/{print $10; exit}')"
[ -n "$WANT" ] && [ "$GOT" = "$WANT" ] || fail "the packaged keyring ($GOT) isn't packaging/linux/ztop-archive-keyring.asc ($WANT)"
echo "Signed by $WANT"

step "The installer is plain sh"
sh -n packaging/linux/install.sh

install -d -m 755 "$SITE/linux"
cp packaging/linux/install.sh "$SITE/install.sh"
cp "$DEB" "$SITE/linux/ztop_latest_$ARCH.deb"
(cd "$(dirname "$DEB")" && sha256sum "$(basename "$DEB")") > "$SITE/linux/ztop_latest_$ARCH.deb.sha256"
chmod -R a+rX "$SITE"
export ZTOP_DOWNLOADS="file://$SITE"

step "A download that doesn't match its checksum isn't installed"
mv "$SITE/linux/ztop_latest_$ARCH.deb.sha256" "$WORK/real.sha256"
echo "0000000000000000000000000000000000000000000000000000000000000000  ztop.deb" > "$SITE/linux/ztop_latest_$ARCH.deb.sha256"
if OUT="$(curl -fsSL "$ZTOP_DOWNLOADS/install.sh" | sh 2>&1)"; then
  echo "$OUT"; fail "the installer accepted a package that doesn't match its checksum"
fi
grep -q "doesn't match its checksum" <<<"$OUT" || { echo "$OUT"; fail "the installer failed without saying the checksum didn't match"; }
dpkg -s ztop >/dev/null 2>&1 && fail "ztop was installed anyway"
mv "$WORK/real.sha256" "$SITE/linux/ztop_latest_$ARCH.deb.sha256"

step "curl ... | sh installs $VERSION"
curl -fsSL "$ZTOP_DOWNLOADS/install.sh" | sh
dpkg -s ztop | grep -q '^Status: install ok installed' || fail "ztop isn't installed"
[ "$(dpkg-query -W -f '${Version}' ztop)" = "$VERSION" ] || fail "the installer didn't install $VERSION"
[ -f /etc/apt/sources.list.d/ztop.sources ] || fail "the installed package didn't add the apt repository"
[ "$(command -v ztop)" = /usr/bin/ztop ] || fail "ztop isn't /usr/bin/ztop"
ztop --version

step "The system agent runs as ztop and pairs one read-only client over loopback"
getent passwd ztop | grep -q ':/usr/sbin/nologin$' || fail "the install didn't create the ztop system user"
[ "$(id -u ztop)" -lt 1000 ] || fail "the ztop user isn't a system user"
if command -v systemd-analyze >/dev/null 2>&1; then
  systemd-analyze verify /usr/lib/systemd/system/ztop-agent.service || fail "systemd rejects the system agent unit"
fi
PAIRED="$(/usr/lib/ztop/pair-system-agent --port 7199)"
PIN="$(sed -n 's/.*"pin":"\(SHA256:[0-9a-f]\{64\}\)".*/\1/p' <<<"$PAIRED")"
TOKEN="$(sed -n 's/.*"token":"\([0-9a-f]\{16,\}\)".*/\1/p' <<<"$PAIRED")"
[ -n "$PIN" ] && [ -n "$TOKEN" ] || fail "the pairing helper printed no pin and token"
[ "$(stat -c %U:%a /var/lib/ztop)" = ztop:700 ] || fail "/var/lib/ztop isn't private to ztop"
[ "$(stat -c %U:%a /var/lib/ztop/ztop/tokens.json)" = ztop:600 ] || fail "the tokens file isn't private to ztop"
grep -q "$TOKEN" /var/lib/ztop/ztop/tokens.json && fail "the tokens file holds the raw token, not its hash"
# Run the agent the way the unit does (CI containers have no systemd).
runuser -u ztop -- env HOME=/var/lib/ztop XDG_CONFIG_HOME=/var/lib/ztop \
  /usr/bin/ztop agent --tls --no-advertise --bind 127.0.0.1 --port 7198 >"$WORK/system-agent.log" 2>&1 &
SYSTEM_AGENT=$!
for _ in $(seq 1 50); do curl -ksf https://127.0.0.1:7198/healthz >/dev/null 2>&1 && break; sleep 0.1; done
grep -q "TLS fingerprint: $PIN" "$WORK/system-agent.log" || fail "the system agent's certificate isn't the one that was paired"
grep -q 'Pairing code' "$WORK/system-agent.log" && fail "the system agent printed a pairing code"
[ "$(curl -ks -o /dev/null -w '%{http_code}' https://127.0.0.1:7198/v1/snapshot)" = 401 ] \
  || fail "the system agent served a snapshot without a token"
[ "$(curl -ks -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $TOKEN" https://127.0.0.1:7198/v1/snapshot)" = 200 ] \
  || fail "the paired token can't read a snapshot"
CONTROL="$(curl -ks -o /dev/null -w '%{http_code}' -X POST -H "Authorization: Bearer $TOKEN" \
  -H 'Content-Type: application/json' -d '{"pids":[1],"action":{"Signal":0}}' https://127.0.0.1:7198/v1/control)"
[ "$CONTROL" = 403 ] || fail "the system agent answered a control request with $CONTROL, not 403"
[ "$(curl -ks -o /dev/null -w '%{http_code}' -H "Authorization: Bearer $TOKEN" https://127.0.0.1:7198/v1/tokens)" = 403 ] \
  || fail "the paired token can list tokens"
kill "$SYSTEM_AGENT"; wait "$SYSTEM_AGENT" 2>/dev/null || true

step "Running it again keeps the same version installed"
curl -fsSL "$ZTOP_DOWNLOADS/install.sh" | sh >/dev/null
[ "$(dpkg-query -W -f '${Version}' ztop)" = "$VERSION" ] || fail "running the installer again changed the version"

step "Build $NEXT and a repository for it, signed with a throwaway key"
export GNUPGHOME="$WORK/gnupg"; mkdir -m 700 "$GNUPGHOME"
gpg --batch --passphrase '' --quick-gen-key "zTop apt check <check@ztop.invalid>" ed25519 sign 1d 2>/dev/null
FPR="$(gpg --list-keys --with-colons | awk -F: '/^fpr/{print $10; exit}')"
gpg --export "$FPR" > "$WORK/check-keyring.gpg"
packaging/build-deb.sh --skip-build --build "$NEXT" --out "$WORK/next" >/dev/null
packaging/build-apt-repo.sh --out "$REPO" --key-id "$FPR" "$WORK"/next/ztop_*.deb
chmod -R a+rX "$REPO"

step "Point the installed package at the local repository"
install -m 644 "$WORK/check-keyring.gpg" /usr/share/keyrings/ztop-check-keyring.gpg
sed -i "s|^URIs: .*|URIs: file://$REPO|; s|^Signed-By: .*|Signed-By: /usr/share/keyrings/ztop-check-keyring.gpg|" \
  /etc/apt/sources.list.d/ztop.sources
only_ztop=(-o Dir::Etc::sourcelist=/dev/null -o Dir::Etc::sourceparts=/etc/apt/sources.list.d)

step "apt verifies the repository and upgrades"
apt-get update -qq "${only_ztop[@]}" || fail "apt couldn't read the signed repository"
CANDIDATE="$(apt-cache policy ztop | sed -n 's/^  Candidate: //p')"
[ "${CANDIDATE##*-}" = "$NEXT" ] || fail "apt offers $CANDIDATE, not build $NEXT"
apt-get install -y -qq --only-upgrade ztop "${only_ztop[@]}" >/dev/null
[ "$(dpkg-query -W -f '${Version}' ztop)" = "${VERSION%-*}-$NEXT" ] || fail "ztop wasn't upgraded to build $NEXT"
grep -q "file://$REPO" /etc/apt/sources.list.d/ztop.sources || fail "the upgrade replaced the changed sources entry"

step "apt refuses an index changed after signing"
sed -i 's/^Label: zTop$/Label: zTop tampered/' "$REPO/dists/stable/InRelease"
rm -rf /var/lib/apt/lists/*ztop-apt-check*
if apt-get update -qq "${only_ztop[@]}" 2>"$WORK/tampered.log" && ! grep -qiE 'not signed|BADSIG|NO_PUBKEY|invalid|verif' "$WORK/tampered.log"; then
  fail "apt accepted a repository whose InRelease no longer matches its signature"
fi
echo "Refused: $(grep -m1 -iE 'not signed|BADSIG|verif|invalid' "$WORK/tampered.log" || true)"

step "Purge"
apt-get purge -y -qq ztop >/dev/null
rm -f /usr/share/keyrings/ztop-check-keyring.gpg
[ ! -e /etc/apt/sources.list.d/ztop.sources ] || fail "purging left the sources entry"
[ ! -e /usr/bin/ztop ] || fail "purging left /usr/bin/ztop"
[ ! -e /var/lib/ztop ] || fail "purging left the system agent's certificate and tokens"
[ ! -e /etc/default/ztop-agent ] || fail "purging left /etc/default/ztop-agent"
echo "ztop installs with the one-line installer, updates through a signed apt repository, and refuses a tampered one."
