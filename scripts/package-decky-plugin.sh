#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PLUGIN_SRC="$REPO_ROOT/decky-plugin"
STAGE_DIR="$(mktemp -d)"
trap 'rm -rf "$STAGE_DIR"' EXIT

if [[ ! -f "$PLUGIN_SRC/dist/index.js" || "${FORCE_BUILD:-0}" == "1" ]]; then
    if command -v npm >/dev/null 2>&1; then
        echo "[package-decky] Building frontend bundle with npm..."
        (cd "$PLUGIN_SRC" && npm install --no-audit --no-fund && npm run build)
    elif command -v podman >/dev/null 2>&1; then
        echo "[package-decky] Building frontend bundle inside rootless Podman (node:20-alpine)..."
        podman run --rm \
            -v "$PLUGIN_SRC:/plugin" \
            -w /plugin \
            docker.io/library/node:20-alpine \
            sh -ec "npm install --no-audit --no-fund && npm run build"
    else
        echo "ERROR: decky-plugin/dist/index.js not found and neither npm nor podman is available." >&2
        exit 1
    fi
fi

PKG_DIR="$STAGE_DIR/steamos-chromoting"
mkdir -p "$PKG_DIR/dist"

cp -f "$PLUGIN_SRC/plugin.json" "$PKG_DIR/plugin.json"
cp -f "$PLUGIN_SRC/package.json" "$PKG_DIR/package.json"
cp -f "$PLUGIN_SRC/main.py" "$PKG_DIR/main.py"
cp -f "$PLUGIN_SRC/dist/index.js" "$PKG_DIR/dist/index.js"
[[ -f "$REPO_ROOT/LICENSE" ]] && cp -f "$REPO_ROOT/LICENSE" "$PKG_DIR/LICENSE"
[[ -f "$REPO_ROOT/README.md" ]] && cp -f "$REPO_ROOT/README.md" "$PKG_DIR/README.md"

mkdir -p "$REPO_ROOT/dist"
OUT_ZIP="$REPO_ROOT/dist/steamos-chromoting-decky.zip"
rm -f "$OUT_ZIP"

python3 - "$STAGE_DIR" "$OUT_ZIP" <<'PY'
import os
import sys
import zipfile

stage_dir, out_zip = sys.argv[1], sys.argv[2]
with zipfile.ZipFile(out_zip, "w", compression=zipfile.ZIP_DEFLATED) as zf:
    for root, dirs, files in os.walk(stage_dir):
        dirs.sort()
        files.sort()
        for name in files:
            full_path = os.path.join(root, name)
            rel_path = os.path.relpath(full_path, stage_dir).replace(os.sep, "/")
            info = zipfile.ZipInfo(rel_path)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = (0o644 & 0xFFFF) << 16
            with open(full_path, "rb") as f:
                zf.writestr(info, f.read())
PY

echo "[package-decky] Created $OUT_ZIP"
