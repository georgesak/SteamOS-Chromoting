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

### Option 1: One-Click Desktop Installer (Recommended)

1. Switch your Steam Deck to **Desktop Mode**.
2. Download **[`Install-SteamOS-Chromoting.desktop`](https://github.com/georgesak/SteamOS-Chromoting/releases/latest/download/Install-SteamOS-Chromoting.desktop)** (or extract the release archive) and **double-click** it.
3. Follow the guided setup wizard:
   - Opens `https://remotedesktop.google.com/headless` in your browser so you can click **Begin → Next → Authorize** and copy the **Debian Linux** command.
   - Paste the copied command directly into the setup dialog, pick a device name, and choose your **6+ digit PIN**.
4. Manage, re-pair, view logs, update, or uninstall anytime from the KDE Application Launcher under **SteamOS Chromoting**.

### Option 2: Terminal / CLI

```bash
# One-line installer (downloads latest release + launches setup wizard):
curl -fsSL https://github.com/georgesak/SteamOS-Chromoting/releases/latest/download/install.sh | bash

# Or manual CLI steps from a release archive:
./cli/steamos-crd install
./cli/steamos-crd setup --code="4/0A..." --pin=123456
./cli/steamos-crd status
```

## Keyboard Chords in Gaming Mode

| Shortcut | Action |
| :--- | :--- |
| `Ctrl + Alt + S` | Press **STEAM** button (opens Steam menu / overlay) |
| `Ctrl + Alt + Q` | Press **Quick Access Menu (`...`)** button |

## License

Released under the [MIT License](LICENSE).
