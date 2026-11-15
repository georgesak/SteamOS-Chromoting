#!/usr/bin/env bash
set -euo pipefail

RUNTIME_DIR="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/steamos-crd"
AUDIO_FIFO="$RUNTIME_DIR/audio.fifo"

mkdir -p "$RUNTIME_DIR"
if [[ ! -p "$AUDIO_FIFO" ]]; then
    rm -f "$AUDIO_FIFO"
    mkfifo -m 0600 "$AUDIO_FIFO"
fi

echo "[steamos-crd-audio] Audio FIFO ready at $AUDIO_FIFO (idle until CRD opens pipe)"

while true; do
    # Opening AUDIO_FIFO for writing blocks at 0% CPU until remoting_me2me_host opens it for reading.
    if command -v parec >/dev/null 2>&1; then
        parec --raw \
            --format=s16le \
            --rate=48000 \
            --channels=2 \
            --latency-msec=20 \
            -d @DEFAULT_MONITOR@ > "$AUDIO_FIFO" 2>/dev/null || true
    elif command -v pw-record >/dev/null 2>&1; then
        pw-record --raw \
            --format=s16 \
            --rate=48000 \
            --channels=2 \
            --latency=20ms \
            --target=@DEFAULT_AUDIO_SINK@ - > "$AUDIO_FIFO" 2>/dev/null || true
    else
        echo "[steamos-crd-audio] WARNING: Neither parec nor pw-record found; sleeping 10s" >&2
        sleep 10
    fi
    sleep 1
done
