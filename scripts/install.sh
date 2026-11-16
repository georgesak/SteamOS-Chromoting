#!/usr/bin/env bash
set -euo pipefail

RELEASE_TARBALL_URL="https://github.com/georgesak/SteamOS-Chromoting/releases/latest/download/steamos-chromoting-linux-x86_64.tar.gz"
RAW_BASE_URL="https://raw.githubusercontent.com/georgesak/SteamOS-Chromoting/main"
TMP_WORK_DIR=""

cleanup() {
    local rc=$?
    if [[ -n "$TMP_WORK_DIR" && -d "$TMP_WORK_DIR" ]]; then
        rm -rf "$TMP_WORK_DIR"
    fi
    if [[ "$rc" -ne 0 ]]; then
        local msg="Installation encountered an error (exit code $rc). Check the terminal output for details."
        if [[ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]]; then
            if command -v kdialog >/dev/null 2>&1; then
                kdialog --title "SteamOS Chromoting Installer" --error "$msg" || true
            elif command -v zenity >/dev/null 2>&1; then
                zenity --error --no-markup --title="SteamOS Chromoting Installer" --text="$msg" --width=460 || true
            fi
        fi
        if [[ -t 1 && -r /dev/tty ]]; then
            echo ""
            read -r -p "Press Enter to close..." _ </dev/tty || true
        fi
    fi
}
trap cleanup EXIT

echo "=== SteamOS-Chromoting Installer ==="

# If executed from a local release folder or built repo, install directly from local files
if [[ -n "${BASH_SOURCE[0]:-}" && -f "${BASH_SOURCE[0]}" ]]; then
    SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
    LOCAL_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
    if [[ -f "$LOCAL_ROOT/cli/steamos-crd" && ( -x "$LOCAL_ROOT/dist/steamos-crd-bridge" || -x "$LOCAL_ROOT/target/release/steamos-crd-bridge" ) ]]; then
        echo "[installer] Found local SteamOS-Chromoting package at $LOCAL_ROOT"
        chmod +x "$LOCAL_ROOT/cli/steamos-crd"
        "$LOCAL_ROOT/cli/steamos-crd" gui-install
        exit 0
    fi
fi

TMP_WORK_DIR="$(mktemp -d "${TMPDIR:-/tmp}/steamos-crd-installer.XXXXXX")"

echo "[installer] Downloading latest SteamOS-Chromoting release from GitHub..."
curl -fL --retry 3 -o "$TMP_WORK_DIR/steamos-chromoting.tar.gz" "$RELEASE_TARBALL_URL"

echo "[installer] Extracting release package..."
tar -xzf "$TMP_WORK_DIR/steamos-chromoting.tar.gz" -C "$TMP_WORK_DIR"

PKG_DIR="$TMP_WORK_DIR/steamos-chromoting"
chmod +x "$PKG_DIR/cli/steamos-crd" "$PKG_DIR/runtime/"*.sh "$PKG_DIR/dist/steamos-crd-bridge"

# Backfill GUI wizard if latest tagged release predates gui-install
if ! grep -q 'gui-install' "$PKG_DIR/cli/steamos-crd" 2>/dev/null; then
    curl -fsSL "$RAW_BASE_URL/cli/steamos-crd" -o "$PKG_DIR/cli/steamos-crd"
    curl -fsSL "$RAW_BASE_URL/runtime/gui-wizard.sh" -o "$PKG_DIR/runtime/gui-wizard.sh"
    chmod +x "$PKG_DIR/cli/steamos-crd" "$PKG_DIR/runtime/gui-wizard.sh"
fi

"$PKG_DIR/cli/steamos-crd" gui-install
