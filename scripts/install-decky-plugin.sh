#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PLUGIN_SRC="$(cd "$SCRIPT_DIR/../decky-plugin" && pwd)"
DECKY_PLUGINS_DIR="${HOME}/homebrew/plugins"
TARGET_DIR="$DECKY_PLUGINS_DIR/steamos-chromoting"

if [[ ! -d "$DECKY_PLUGINS_DIR" ]]; then
    echo "ERROR: Decky Loader plugins directory not found at $DECKY_PLUGINS_DIR." >&2
    echo "Install Decky Loader first (https://decky.xyz), then re-run this script." >&2
    exit 1
fi

echo "[decky-install] Building Decky frontend bundle inside rootless Podman (node:20-alpine)..."
podman run --rm \
    -v "$PLUGIN_SRC:/plugin" \
    -w /plugin \
    docker.io/library/node:20-alpine \
    sh -ec "npm install --no-audit --no-fund && npm run build"

echo "[decky-install] Installing plugin to $TARGET_DIR (requires sudo for ~/homebrew/plugins)..."
sudo mkdir -p "$TARGET_DIR/dist"
sudo cp -f "$PLUGIN_SRC/plugin.json" "$PLUGIN_SRC/package.json" "$PLUGIN_SRC/main.py" "$TARGET_DIR/"
sudo cp -f "$PLUGIN_SRC/dist/index.js" "$TARGET_DIR/dist/index.js"
sudo systemctl restart plugin_loader.service

echo "[decky-install] SteamOS Chromoting Decky plugin installed and loaded!"
