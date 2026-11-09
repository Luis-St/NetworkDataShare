#!/bin/sh
# Installs NetworkDataShare: copies the binary next to this script into the installation directory
# and runs its setup from there with the configuration below, so the systemd service points at the
# installed copy. Edit the values below as needed, run ./network-data-share --help for their meaning.
set -eu

INSTALL_DIR="$HOME/.local/bin"

BIND="0.0.0.0:8080"
INVITE_TIMEOUT=60
RECONNECT_GRACE=10
MAX_CLIENTS_PER_IP=32

name=network-data-share
src="$(cd "$(dirname "$0")" && pwd)/$name"

if [ ! -x "$src" ]; then
    echo "binary not found: $src" >&2
    exit 1
fi

mkdir -p "$INSTALL_DIR"
# Copy to a temporary name first, so a running service never sees a half-written binary.
cp "$src" "$INSTALL_DIR/$name.new"
chmod 755 "$INSTALL_DIR/$name.new"
mv -f "$INSTALL_DIR/$name.new" "$INSTALL_DIR/$name"
echo "Installed $INSTALL_DIR/$name"

exec "$INSTALL_DIR/$name" --setup \
    --bind "$BIND" \
    --invite-timeout "$INVITE_TIMEOUT" \
    --reconnect-grace "$RECONNECT_GRACE" \
    --max-clients-per-ip "$MAX_CLIENTS_PER_IP"
