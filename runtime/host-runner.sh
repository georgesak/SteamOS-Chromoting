#!/usr/bin/env bash
set -euo pipefail

SETTINGS_FILE="${XDG_CONFIG_HOME:-$HOME/.config}/steamos-chromoting/settings.env"
if [[ -f "$SETTINGS_FILE" ]]; then
    set -a
    # shellcheck disable=SC1090
    source "$SETTINGS_FILE"
    set +a
fi

INSTALL_DIR="${STEAMOS_CRD_DIR:-$HOME/.local/share/steamos-chromoting}"
CRD_OPT="$INSTALL_DIR/opt/google/chrome-remote-desktop"
CONFIG_DIR="$HOME/.config/chrome-remote-desktop"
CRD_DISPLAY="${STEAMOS_CRD_DISPLAY:-:20}"
RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/steamos-crd"
AUDIO_FIFO="$RUNTIME_DIR/audio.fifo"

mkdir -p "$RUNTIME_DIR"
if [[ ! -p "$AUDIO_FIFO" ]]; then
    rm -f "$AUDIO_FIFO"
    mkfifo -m 0600 "$AUDIO_FIFO"
fi

HOST_CONFIG="$(find "$CONFIG_DIR" -maxdepth 1 -name 'host#*.json' 2>/dev/null | head -n 1 || true)"
if [[ -z "$HOST_CONFIG" || ! -f "$HOST_CONFIG" ]]; then
    echo "[steamos-crd-host] No host configuration found in $CONFIG_DIR." >&2
    echo "[steamos-crd-host] Run 'steamos-crd setup --code=...' to register with Google Chrome Remote Desktop." >&2
    exit 1
fi

HOST_BIN="/opt/google/chrome-remote-desktop/chrome-remote-desktop-host"
if [[ ! -x "$CRD_OPT/chrome-remote-desktop-host" && -x "$CRD_OPT/remoting_me2me_host" ]]; then
    HOST_BIN="/opt/google/chrome-remote-desktop/remoting_me2me_host"
fi

# Wait up to 10s for Xvfb socket to appear
DISP_NUM="${CRD_DISPLAY#:}"
for _ in $(seq 1 40); do
    if [[ -S "/tmp/.X11-unix/X${DISP_NUM}" ]]; then
        break
    fi
    sleep 0.25
done

# Ensure evdev XKB rules are active on Xvfb so X11 keycodes == evdev + 8
if command -v setxkbmap >/dev/null 2>&1 || [[ -x "$INSTALL_DIR/sysroot/bin/setxkbmap" ]]; then
    PATH="$INSTALL_DIR/sysroot/bin:$PATH" \
    LD_LIBRARY_PATH="$INSTALL_DIR/sysroot/lib:${LD_LIBRARY_PATH:-}" \
    setxkbmap -display "$CRD_DISPLAY" -rules evdev -model pc105 -layout us 2>/dev/null || true
fi

export DISPLAY="$CRD_DISPLAY"
unset WAYLAND_DISPLAY
unset XAUTHORITY
unset SSH_CONNECTION
export GDK_BACKEND=x11
export QT_QPA_PLATFORM=xcb
export OZONE_PLATFORM=x11
export CHROME_REMOTE_DESKTOP_SESSION=1
if [[ "${STEAMOS_CRD_AUDIO_ENABLED:-1}" == "0" ]]; then
    unset CHROME_REMOTE_DESKTOP_AUDIO_PIPE
else
    export CHROME_REMOTE_DESKTOP_AUDIO_PIPE="$AUDIO_FIFO"
fi

echo "[steamos-crd-host] Launching $(basename "$HOST_BIN") on DISPLAY=$DISPLAY using $(basename "$HOST_CONFIG")..."
exec "$INSTALL_DIR/runtime/bwrap-crd.sh" \
    "$HOST_BIN" \
    --host-config=- \
    ${STEAMOS_CRD_EXTRA_HOST_ARGS:-} < "$HOST_CONFIG"
