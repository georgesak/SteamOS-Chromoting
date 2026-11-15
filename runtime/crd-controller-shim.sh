#!/usr/bin/env bash
# Rootless controller shim replacing /opt/google/chrome-remote-desktop/chrome-remote-desktop
# inside the Bubblewrap namespace so `start-host` works with zero sudo/group prompts.
set -euo pipefail

CONFIG_DIR="$HOME/.config/chrome-remote-desktop"
mkdir -p "$CONFIG_DIR"
chmod 0700 "$CONFIG_DIR"

HOST_HASH="$(hostname | md5sum | awk '{print $1}')"
DEFAULT_CONFIG="$CONFIG_DIR/host#${HOST_HASH}.json"

find_config() {
    local found
    found="$(find "$CONFIG_DIR" -maxdepth 1 -name 'host#*.json' 2>/dev/null | head -n 1 || true)"
    if [[ -n "$found" ]]; then
        echo "$found"
    else
        echo "$DEFAULT_CONFIG"
    fi
}

is_running() {
    systemctl --user is-active --quiet steamos-crd-host.service 2>/dev/null
}

ACTION=""
CONFIG_PATH=""

for arg in "$@"; do
    case "$arg" in
        --check-running|--get-status|--start|--enable-and-start|--stop|--reload|--add-user|--SaveConfig|--save-config)
            ACTION="$arg"
            ;;
        --config=*)
            CONFIG_PATH="${arg#--config=}"
            ;;
    esac
done

case "$ACTION" in
    --add-user)
        # Rootless user-space daemon; no system group membership needed.
        exit 0
        ;;
    --check-running)
        if is_running; then
            exit 0
        else
            exit 1
        fi
        ;;
    --get-status)
        CFG="$(find_config)"
        if [[ ! -f "$CFG" ]]; then
            echo "NOT_IMPLEMENTED" >/dev/null
            echo "STOPPED"
        elif is_running; then
            echo "STARTED"
        else
            echo "STOPPED"
        fi
        exit 0
        ;;
    --save-config|--SaveConfig)
        TARGET_CFG="${CONFIG_PATH:-$DEFAULT_CONFIG}"
        umask 0077
        cat > "$TARGET_CFG"
        exit 0
        ;;
    --start|--enable-and-start|--reload)
        # If stdin is piped (some start-host versions pipe JSON on --start), capture it if non-empty
        if [[ ! -t 0 ]]; then
            TMP_IN="$(mktemp)"
            cat > "$TMP_IN" || true
            if [[ -s "$TMP_IN" ]]; then
                TARGET_CFG="${CONFIG_PATH:-$DEFAULT_CONFIG}"
                umask 0077
                mv "$TMP_IN" "$TARGET_CFG"
            else
                rm -f "$TMP_IN"
            fi
        fi
        systemctl --user daemon-reload || true
        systemctl --user enable --now steamos-crd.target || true
        systemctl --user restart steamos-crd-host.service || true
        exit 0
        ;;
    --stop)
        systemctl --user stop steamos-crd-host.service || true
        exit 0
        ;;
    *)
        exit 0
        ;;
esac
