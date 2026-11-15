#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
CACHE_DIR="${HOME}/.cache/steamos-chromoting-builder"
BUILDER_IMG="localhost/steamos-crd-builder:latest"

mkdir -p "$CACHE_DIR/cargo-registry" "$CACHE_DIR/cargo-git" "$CACHE_DIR/target"

if ! podman image exists "$BUILDER_IMG"; then
    echo "[builder] Creating cached Rust + PipeWire builder image ($BUILDER_IMG)..."
    podman build -t "$BUILDER_IMG" -f - "$REPO_ROOT" <<'EOF'
FROM docker.io/library/archlinux:latest
RUN pacman -Sy --noconfirm --needed base-devel rust pkgconf pipewire libpipewire && pacman -Scc --noconfirm
EOF
fi

echo "[builder] Compiling steamos-crd-bridge (release)..."
podman run --rm \
    -v "$REPO_ROOT:/work" \
    -v "$CACHE_DIR/cargo-registry:/root/.cargo/registry" \
    -v "$CACHE_DIR/cargo-git:/root/.cargo/git" \
    -v "$CACHE_DIR/target:/work/target" \
    -w /work \
    "$BUILDER_IMG" \
    cargo build --release

mkdir -p "$REPO_ROOT/dist"
cp -f "$CACHE_DIR/target/release/steamos-crd-bridge" "$REPO_ROOT/dist/steamos-crd-bridge"
chmod +x "$REPO_ROOT/dist/steamos-crd-bridge"
echo "[builder] Built binary at $REPO_ROOT/dist/steamos-crd-bridge"
