#!/usr/bin/env bash
set -euo pipefail

INSTALL_DIR="${STEAMOS_CRD_DIR:-$HOME/.local/share/steamos-chromoting}"
CRD_OPT="$INSTALL_DIR/opt/google/chrome-remote-desktop"
CRD_DEB_URL="https://dl.google.com/linux/direct/chrome-remote-desktop_current_amd64.deb"
TMP_DIR="$(mktemp -d "${TMPDIR:-/tmp}/steamos-crd-fetch.XXXXXX")"

cleanup() {
    rm -rf "$TMP_DIR"
}
trap cleanup EXIT

echo "[steamos-crd] Preparing install directory at: $INSTALL_DIR"
mkdir -p "$INSTALL_DIR/opt/google" "$INSTALL_DIR/sysroot/bin" "$INSTALL_DIR/sysroot/lib" "$INSTALL_DIR/bin"

if [[ "${STEAMOS_CRD_FORCE_FETCH:-0}" == "1" || ( ! -x "$CRD_OPT/chrome-remote-desktop-host" && ! -x "$CRD_OPT/remoting_me2me_host" ) ]]; then
    echo "[steamos-crd] Downloading official Chrome Remote Desktop (.deb)..."
    curl -fL --retry 3 -o "$TMP_DIR/crd.deb" "$CRD_DEB_URL"

    echo "[steamos-crd] Extracting Chrome Remote Desktop binaries..."
    mkdir -p "$TMP_DIR/extracted"
    if command -v bsdtar >/dev/null 2>&1; then
        bsdtar -xf "$TMP_DIR/crd.deb" -C "$TMP_DIR"
        DATA_TAR="$(find "$TMP_DIR" -maxdepth 1 -name 'data.tar.*' | head -n 1)"
        bsdtar -xf "$DATA_TAR" -C "$TMP_DIR/extracted"
    elif command -v ar >/dev/null 2>&1; then
        (cd "$TMP_DIR" && ar x crd.deb)
        DATA_TAR="$(find "$TMP_DIR" -maxdepth 1 -name 'data.tar.*' | head -n 1)"
        tar -xf "$DATA_TAR" -C "$TMP_DIR/extracted"
    else
        echo "ERROR: Neither bsdtar nor ar is available to unpack .deb" >&2
        exit 1
    fi

    rm -rf "$CRD_OPT"
    mv "$TMP_DIR/extracted/opt/google/chrome-remote-desktop" "$CRD_OPT"
else
    echo "[steamos-crd] Official CRD binaries already present in $CRD_OPT (set STEAMOS_CRD_FORCE_FETCH=1 to re-download)."
fi

HOST_BIN="$CRD_OPT/chrome-remote-desktop-host"
if [[ ! -e "$HOST_BIN" && -e "$CRD_OPT/remoting_me2me_host" ]]; then
    HOST_BIN="$CRD_OPT/remoting_me2me_host"
fi

chmod +x "$HOST_BIN" "$CRD_OPT/start-host" "$CRD_OPT/chrome-remote-desktop"
[[ -f "$CRD_OPT/libremoting_core.so" ]] && chmod +x "$CRD_OPT/libremoting_core.so" || true

# Check if Xvfb or any dynamic libraries required by the CRD host are missing on the SteamOS host
NEED_SYSROOT=0
if ! command -v Xvfb >/dev/null 2>&1 && [[ ! -x "$INSTALL_DIR/sysroot/bin/Xvfb" ]]; then
    echo "[steamos-crd] Xvfb not found on host; bundling Xvfb into user sysroot..."
    NEED_SYSROOT=1
fi

LDD_TARGETS=("$HOST_BIN" "$CRD_OPT/start-host")
[[ -f "$CRD_OPT/libremoting_core.so" ]] && LDD_TARGETS+=("$CRD_OPT/libremoting_core.so")

MISSING_LIBS="$(LD_LIBRARY_PATH="$CRD_OPT:$INSTALL_DIR/sysroot/lib:${LD_LIBRARY_PATH:-}" \
    ldd "${LDD_TARGETS[@]}" 2>/dev/null \
    | awk '/not found/ {print $1}' | sort -u || true)"

if [[ -n "$MISSING_LIBS" ]]; then
    echo "[steamos-crd] Missing shared libraries detected:"
    echo "$MISSING_LIBS" | sed 's/^/  - /'
    NEED_SYSROOT=1
fi

if [[ "$NEED_SYSROOT" -eq 1 ]]; then
    if ! command -v podman >/dev/null 2>&1; then
        echo "WARNING: podman not found to bundle Xvfb/libs automatically." >&2
        echo "Please ensure Xvfb and missing libraries above are available." >&2
    else
        echo "[steamos-crd] Extracting Xvfb and XKB tools via rootless Podman (archlinux:latest)..."
        podman run --rm \
            -v "$INSTALL_DIR/sysroot:/out" \
            docker.io/library/archlinux:latest \
            bash -ec '
                pacman -Sy --noconfirm --needed xorg-server-xvfb libxss xorg-xkbcomp xkeyboard-config xorg-setxkbmap xorg-xrandr xorg-xauth >/dev/null
                cp -L /usr/bin/Xvfb /usr/bin/xkbcomp /usr/bin/setxkbmap /usr/bin/xrandr /usr/bin/xauth /out/bin/
                chmod +x /out/bin/*
                mkdir -p /out/share
                cp -rL /usr/share/X11 /out/share/
                for lib in $(ldd /usr/bin/Xvfb | awk "/=> \// {print \$3}"); do
                    case "$(basename "$lib")" in
                        libc.so*|libm.so*|libdl.so*|libpthread.so*|librt.so*|ld-linux*) ;;
                        *) cp -Ln "$lib" /out/lib/ || true ;;
                    esac
                done
                for extra in /usr/lib/libXss.so* /usr/lib/libXtst.so* /usr/lib/libXdamage.so* /usr/lib/libXrandr.so*; do
                    [[ -e "$extra" ]] && cp -Ln "$extra" /out/lib/ || true
                done
            '
    fi
fi

echo "[steamos-crd] Official CRD runtime installed successfully in $INSTALL_DIR"
