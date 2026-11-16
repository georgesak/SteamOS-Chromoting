#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
STAGE_DIR="$(mktemp -d)"
trap 'rm -rf "$STAGE_DIR"' EXIT

PKG_ROOT="$STAGE_DIR/steamos-chromoting"
mkdir -p "$PKG_ROOT/dist" "$PKG_ROOT/runtime/systemd" "$PKG_ROOT/cli" "$PKG_ROOT/scripts"

cp -f "$REPO_ROOT/README.md" "$PKG_ROOT/"
[[ -f "$REPO_ROOT/LICENSE" ]] && cp -f "$REPO_ROOT/LICENSE" "$PKG_ROOT/"
cp -f "$REPO_ROOT/Install-SteamOS-Chromoting.desktop" "$PKG_ROOT/"
cp -f "$REPO_ROOT/scripts/install.sh" "$PKG_ROOT/scripts/"
cp -f "$REPO_ROOT/cli/steamos-crd" "$PKG_ROOT/cli/"
cp -f "$REPO_ROOT/runtime/"*.sh "$PKG_ROOT/runtime/"
[[ -f "$REPO_ROOT/runtime/pam-chrome-remote-desktop" ]] && cp -f "$REPO_ROOT/runtime/pam-chrome-remote-desktop" "$PKG_ROOT/runtime/"
cp -f "$REPO_ROOT/runtime/systemd/"* "$PKG_ROOT/runtime/systemd/"

if [[ -x "$REPO_ROOT/dist/steamos-crd-bridge" ]]; then
    cp -f "$REPO_ROOT/dist/steamos-crd-bridge" "$PKG_ROOT/dist/steamos-crd-bridge"
elif [[ -x "$REPO_ROOT/target/release/steamos-crd-bridge" ]]; then
    cp -f "$REPO_ROOT/target/release/steamos-crd-bridge" "$PKG_ROOT/dist/steamos-crd-bridge"
else
    echo "ERROR: Built steamos-crd-bridge binary not found in dist/ or target/release/" >&2
    exit 1
fi

chmod +x \
    "$PKG_ROOT/Install-SteamOS-Chromoting.desktop" \
    "$PKG_ROOT/scripts/install.sh" \
    "$PKG_ROOT/cli/steamos-crd" \
    "$PKG_ROOT/runtime/"*.sh \
    "$PKG_ROOT/dist/steamos-crd-bridge"

mkdir -p "$REPO_ROOT/dist"
cp -f "$REPO_ROOT/Install-SteamOS-Chromoting.desktop" "$REPO_ROOT/dist/Install-SteamOS-Chromoting.desktop"
cp -f "$REPO_ROOT/scripts/install.sh" "$REPO_ROOT/dist/install.sh"
chmod +x "$REPO_ROOT/dist/Install-SteamOS-Chromoting.desktop" "$REPO_ROOT/dist/install.sh"
tar -czf "$REPO_ROOT/dist/steamos-chromoting-linux-x86_64.tar.gz" -C "$STAGE_DIR" steamos-chromoting
echo "[package] Created $REPO_ROOT/dist/steamos-chromoting-linux-x86_64.tar.gz"
