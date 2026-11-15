#!/usr/bin/env bash
set -euo pipefail

INSTALL_DIR="${STEAMOS_CRD_DIR:-$HOME/.local/share/steamos-chromoting}"
CRD_OPT="$INSTALL_DIR/opt/google/chrome-remote-desktop"

if [[ ! -x "$CRD_OPT/chrome-remote-desktop-host" && ! -x "$CRD_OPT/remoting_me2me_host" ]]; then
    echo "ERROR: CRD runtime not installed at $CRD_OPT. Run 'steamos-crd install' first." >&2
    exit 1
fi

BWRAP_ARGS=(
    --ro-bind / /
    --dev /dev
    --proc /proc
    --bind /tmp /tmp
    --bind /run /run
    --bind "$HOME" "$HOME"
    --tmpfs /opt
)

# Preserve existing /opt entries if present on SteamOS
for entry in /opt/*; do
    if [[ -e "$entry" && "$(basename "$entry")" != "google" ]]; then
        BWRAP_ARGS+=(--ro-bind "$entry" "$entry")
    fi
done

if [[ -d /opt/google ]]; then
    for gentry in /opt/google/*; do
        if [[ -e "$gentry" && "$(basename "$gentry")" != "chrome-remote-desktop" ]]; then
            BWRAP_ARGS+=(--ro-bind "$gentry" "$gentry")
        fi
    done
fi

BWRAP_ARGS+=(
    --bind "$CRD_OPT" /opt/google/chrome-remote-desktop
    --ro-bind "$INSTALL_DIR/runtime/crd-controller-shim.sh" /opt/google/chrome-remote-desktop/chrome-remote-desktop
    --setenv PATH "$INSTALL_DIR/sysroot/bin:$INSTALL_DIR/bin:$PATH"
    --setenv LD_LIBRARY_PATH "$INSTALL_DIR/sysroot/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
)

exec bwrap "${BWRAP_ARGS[@]}" "$@"
