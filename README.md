# SteamOS-Chromoting

A rootless, SteamOS-update-persistent **Chrome Remote Desktop** host that works seamlessly across **both Gaming Mode (Gamescope)** and **Desktop Mode (KDE Plasma X11 & Wayland)** — including live mode switches without dropping your remote connection.

## Highlights

- **Works with Official Chrome Remote Desktop Clients**: Uses Google's official `remoting_me2me_host` and `start-host` binaries inside a lightweight, rootless **Bubblewrap (`bwrap`)** user namespace so `remotedesktop.google.com` and mobile apps work out of the box.
- **Survives Gaming ↔ Desktop Mode Transitions**: Runs `remoting_me2me_host` against a persistent virtual display (`Xvfb :20`) fed by `steamos-crd-bridge` (Rust), keeping WebRTC alive while SteamOS tears down and restarts Gamescope or KDE Plasma.
- **Universal Video & Input**:
  - **Gaming Mode**: Captures Gamescope's native PipeWire stream (`media.name = "gamescope"`) with zero portal prompts.
  - **Desktop Mode**: Captures Plasma X11 via `XShm`/`XDamage` and Plasma Wayland via `xdg-desktop-portal` ScreenCast (`restore_token` cached).
  - **Input**: Translates X11 events into kernel `/dev/uinput` (unprivileged via SteamOS `uaccess` rules) for mouse, keyboard, and **STEAM** / **QAM (`...`)** hardware button chords.
  - **Clipboard**: Bidirectional UTF-8 clipboard sync across Gamescope Xwayland displays (`:0` & `:1`), KDE Plasma X11, and KDE Plasma Wayland (`org.kde.klipper`).
  - **Audio**: Streams `@DEFAULT_MONITOR@` (`48kHz s16le`) across both modes without muting or hijacking Steam Deck speakers.
- **100% Rootless & Update-Resilient**: Everything lives under `~/.local/share/steamos-chromoting` and `~/.config/systemd/user/` — untouched by SteamOS A/B rootfs updates.

## Quick Start (on SteamOS)

```bash
# 1. Install runtime & user services
./cli/steamos-crd install

# 2. Authorize via https://remotedesktop.google.com/headless ("Debian Linux" copy command or --code)
./cli/steamos-crd setup --code="4/0A..." --pin=123456

# 3. Check status
./cli/steamos-crd status
```

## Keyboard Chords in Gaming Mode

| Shortcut | Action |
| :--- | :--- |
| `Ctrl + Alt + S` | Press **STEAM** button (opens Steam menu / overlay) |
| `Ctrl + Alt + Q` | Press **Quick Access Menu (`...`)** button |

## License

Released under the [MIT License](LICENSE).
