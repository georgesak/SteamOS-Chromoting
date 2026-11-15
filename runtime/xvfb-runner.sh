#!/usr/bin/env bash
set -euo pipefail

INSTALL_DIR="${STEAMOS_CRD_DIR:-$HOME/.local/share/steamos-chromoting}"
CRD_DISPLAY="${STEAMOS_CRD_DISPLAY:-:20}"
CRD_RES="${STEAMOS_CRD_RESOLUTION:-1920x1080x24}"

export PATH="$INSTALL_DIR/sysroot/bin:$PATH"
export LD_LIBRARY_PATH="$INSTALL_DIR/sysroot/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"

XVFB_BIN="$(command -v Xvfb || true)"
if [[ -z "$XVFB_BIN" ]]; then
    echo "ERROR: Xvfb binary not found in PATH or $INSTALL_DIR/sysroot/bin" >&2
    exit 1
fi

EXTRA_XKB_ARGS=()
if [[ -d "$INSTALL_DIR/sysroot/share/X11/xkb" && ! -d /usr/share/X11/xkb ]]; then
    EXTRA_XKB_ARGS+=(-xkbdir "$INSTALL_DIR/sysroot/share/X11/xkb")
fi

# Clean stale lock if previous Xvfb died uncleanly
DISP_NUM="${CRD_DISPLAY#:}"
LOCK_FILE="/tmp/.X${DISP_NUM}-lock"
if [[ -f "$LOCK_FILE" ]]; then
    LOCK_PID="$(tr -d '[:space:]' < "$LOCK_FILE" || true)"
    if [[ -n "$LOCK_PID" ]] && ! kill -0 "$LOCK_PID" 2>/dev/null; then
        rm -f "$LOCK_FILE" "/tmp/.X11-unix/X${DISP_NUM}"
    fi
fi

echo "[steamos-crd-xvfb] Starting Xvfb on $CRD_DISPLAY ($CRD_RES)..."
exec "$XVFB_BIN" "$CRD_DISPLAY" \
    -screen 0 "$CRD_RES" \
    -nolisten tcp \
    -noreset \
    +extension RANDR \
    +extension DAMAGE \
    +extension MIT-SHM \
    +extension XTEST \
    +extension XFIXES \
    "${EXTRA_XKB_ARGS[@]}"
