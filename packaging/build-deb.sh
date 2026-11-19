#!/bin/bash
# SPDX-License-Identifier: MIT OR Apache-2.0
#
# Build .deb packages for zTop (amd64 and arm64).
#
# Usage:
#     ./build-deb.sh [version]
#
# The script builds the ztop binary for the target architecture and
# packages it into a .deb file with a systemd system service that runs
# the agent as the dedicated, unprivileged `ztop` user (DESIGN.md). Files
# go under /usr, never /usr/local, which Debian leaves to the local
# administrator.

set -euo pipefail

VERSION="${1:-0.1.0}"
ARCH="${2:-amd64}"
BUILD_DIR="$(mktemp -d)"
PACKAGE_NAME="ztop"
# No email address: the project publishes none (AGENTS.md). Questions go
# to the repository's issues.
MAINTAINER="zTop maintainers"
DESCRIPTION="A lightweight system monitor for macOS and Linux"

echo "Building zTop ${VERSION} for ${ARCH}..."

# Determine the Rust target triple
case "$ARCH" in
    amd64)
        TARGET="x86_64-unknown-linux-gnu"
        ;;
    arm64)
        TARGET="aarch64-unknown-linux-gnu"
        ;;
    *)
        echo "Unknown architecture: $ARCH"
        exit 1
        ;;
esac

# Build the binary
cd "$(dirname "$0")/.."
cargo build --release --target "$TARGET"

# Create the package structure
PKG_DIR="$BUILD_DIR/${PACKAGE_NAME}_${VERSION}_${ARCH}"
mkdir -p "$PKG_DIR/DEBIAN"
mkdir -p "$PKG_DIR/usr/bin"
mkdir -p "$PKG_DIR/lib/systemd/system"
mkdir -p "$PKG_DIR/usr/share/man/man1"
mkdir -p "$PKG_DIR/usr/share/doc/ztop"

# Copy the binary
cp "target/${TARGET}/release/ztop" "$PKG_DIR/usr/bin/ztop"
chmod 755 "$PKG_DIR/usr/bin/ztop"

# Copy the systemd system service
cp "packaging/systemd/ztop-agent-system.service" "$PKG_DIR/lib/systemd/system/ztop-agent.service"
chmod 644 "$PKG_DIR/lib/systemd/system/ztop-agent.service"

# Copy the man page, compressed as Debian expects
gzip -9n -c "packaging/man/ztop.1" > "$PKG_DIR/usr/share/man/man1/ztop.1.gz"
chmod 644 "$PKG_DIR/usr/share/man/man1/ztop.1.gz"

# The license texts
cp LICENSE-MIT LICENSE-APACHE "$PKG_DIR/usr/share/doc/ztop/"
chmod 644 "$PKG_DIR/usr/share/doc/ztop/"*

# Create the control file
cat > "$PKG_DIR/DEBIAN/control" << EOF
Package: ${PACKAGE_NAME}
Version: ${VERSION}
Architecture: ${ARCH}
Maintainer: ${MAINTAINER}
Homepage: https://github.com/BITS-Freeware/zTop
Description: ${DESCRIPTION}
Section: utils
Priority: optional
Depends: libc6, adduser
EOF

# postinst: create the system user (remembering that the package did, so
# purge removes only a user it made), then enable and start the service.
cat > "$PKG_DIR/DEBIAN/postinst" << 'EOF'
#!/bin/sh
set -e
if [ "$1" = "configure" ]; then
    if ! getent passwd ztop >/dev/null; then
        adduser --system --group --home /var/lib/ztop --no-create-home \
            --shell /usr/sbin/nologin --gecos "zTop agent" ztop >/dev/null
        mkdir -p /var/lib/ztop
        chown ztop:ztop /var/lib/ztop
        chmod 0700 /var/lib/ztop
        touch /var/lib/ztop/.created-by-package
    fi
    if [ -d /run/systemd/system ]; then
        systemctl daemon-reload || true
        systemctl enable ztop-agent.service || true
        systemctl restart ztop-agent.service || true
    fi
fi
EOF
chmod 755 "$PKG_DIR/DEBIAN/postinst"

# prerm: stop the service before its binary goes.
cat > "$PKG_DIR/DEBIAN/prerm" << 'EOF'
#!/bin/sh
set -e
if [ -d /run/systemd/system ]; then
    systemctl stop ztop-agent.service 2>/dev/null || true
    if [ "$1" = "remove" ]; then
        systemctl disable ztop-agent.service 2>/dev/null || true
    fi
fi
EOF
chmod 755 "$PKG_DIR/DEBIAN/prerm"

# postrm: on purge, delete the agent's state, and the user only when this
# package created it. Nothing outside /var/lib/ztop is ever removed.
cat > "$PKG_DIR/DEBIAN/postrm" << 'EOF'
#!/bin/sh
set -e
if [ -d /run/systemd/system ]; then
    systemctl daemon-reload 2>/dev/null || true
fi
if [ "$1" = "purge" ]; then
    created=no
    if [ -e /var/lib/ztop/.created-by-package ]; then
        created=yes
    fi
    rm -rf /var/lib/ztop
    if [ "$created" = yes ] && getent passwd ztop >/dev/null; then
        deluser --system ztop >/dev/null 2>&1 || true
    fi
fi
EOF
chmod 755 "$PKG_DIR/DEBIAN/postrm"

# Build the .deb package in the repository root
dpkg-deb --build "$PKG_DIR" "${PACKAGE_NAME}_${VERSION}_${ARCH}.deb"

echo "Built: ${PACKAGE_NAME}_${VERSION}_${ARCH}.deb"

# Clean up the staging directory (keep the .deb)
rm -rf "$PKG_DIR"
