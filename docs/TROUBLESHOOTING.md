# Troubleshooting SteamOS-Chromoting

## 1. OAuth Error During `steamos-crd setup`
- Headless authorization codes generated at `https://remotedesktop.google.com/headless` are **single-use** and expire after a few minutes.
- Generate a fresh Debian Linux command at `https://remotedesktop.google.com/headless` and pass the new `--code="4/0A..."` to `steamos-crd setup`.

## 2. Steam Overlay or Quick Access Menu (`...`) Missing in Gaming Mode
- `steamos-crd-bridge` automatically sets `GAMESCOPE_COMPOSITE_FORCE=1` on `:0` whenever a remote client connects so Gamescope merges overlay planes into the PipeWire capture stream.
- Use `Ctrl + Alt + S` for the **STEAM** button and `Ctrl + Alt + Q` for the **Quick Access Menu (`...`)**.

## 3. First-Time Connection in Desktop Mode (Wayland)
- If your SteamOS Desktop Mode uses **KDE Plasma X11** (`plasmax11.desktop`, the default `"Switch to Desktop"` target), capture works immediately with zero prompts.
- If you switch Desktop Mode to **KDE Plasma Wayland** (`plasma.desktop`), KDE's `xdg-desktop-portal-kde` displays a one-time *"Share your screen"* approval dialog on the local screen. Check **"Allow restoring on future sessions"** — `steamos-crd-bridge` caches the `restore_token` in `~/.config/steamos-chromoting/portal_token` so future sessions connect unattended.

## 4. Surviving SteamOS Updates
- All binaries, Bubblewrap sysroot files, credentials, and `systemd --user` units reside inside `/home/deck` (`~/.local/share/steamos-chromoting`, `~/.config/chrome-remote-desktop`, and `~/.config/systemd/user`), so SteamOS A/B root partition updates leave the entire installation intact.

## 5. Checking Logs
```bash
steamos-crd status
steamos-crd logs          # All units
steamos-crd logs bridge   # Capture & uinput bridge
steamos-crd logs host     # Official Google CRD WebRTC host
```
