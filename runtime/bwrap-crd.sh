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

PAM_CRD_FILE="$INSTALL_DIR/runtime/pam-chrome-remote-desktop"
if [[ ! -f "$PAM_CRD_FILE" ]]; then
    mkdir -p "$INSTALL_DIR/runtime"
    cat > "$PAM_CRD_FILE" <<'EOF'
#%PAM-1.0
auth      required  pam_permit.so
account   required  pam_permit.so
password  required  pam_permit.so
session   required  pam_permit.so
EOF
fi

# Overlay /etc/pam.d/chrome-remote-desktop inside the rootless container so PAM account check
# doesn't fall back to SteamOS's /etc/pam.d/other (pam_deny.so).
if [[ -d /etc/pam.d ]]; then
    BWRAP_ARGS+=(--tmpfs /etc/pam.d)
    for pentry in /etc/pam.d/*; do
        if [[ -e "$pentry" && "$(basename "$pentry")" != "chrome-remote-desktop" ]]; then
            BWRAP_ARGS+=(--ro-bind "$pentry" "$pentry")
        fi
    done
    BWRAP_ARGS+=(--ro-bind "$PAM_CRD_FILE" /etc/pam.d/chrome-remote-desktop)
fi

BWRAP_ARGS+=(
    --bind "$CRD_OPT" /opt/google/chrome-remote-desktop
    --ro-bind "$INSTALL_DIR/runtime/crd-controller-shim.sh" /opt/google/chrome-remote-desktop/chrome-remote-desktop
    --setenv PATH "$INSTALL_DIR/sysroot/bin:$INSTALL_DIR/bin:$PATH"
    --setenv LD_LIBRARY_PATH "$INSTALL_DIR/sysroot/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
)

exec bwrap "${BWRAP_ARGS[@]}" "$@"
