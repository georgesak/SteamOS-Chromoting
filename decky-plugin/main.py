import asyncio
import base64
import glob
import hashlib
import hmac
import json
import os
try:
    import pwd
except ImportError:
    pwd = None  # type: ignore[assignment]
import re
import shutil
import socket
import subprocess
import tempfile
import urllib.request
from typing import Any, Dict, List, Optional, Tuple

import decky

RELEASE_TARBALL_URL = (
    "https://github.com/georgesak/SteamOS-Chromoting/releases/latest/download/"
    "steamos-chromoting-linux-x86_64.tar.gz"
)
DEFAULT_REDIRECT_URL = "https://remotedesktop.google.com/_/oauthredirect"

VALID_RESOLUTIONS = {
    "1280x800x24",
    "1920x1080x24",
    "1920x1200x24",
    "2560x1440x24",
}
VALID_FPS = {15, 30, 45, 60, 90, 120}
VALID_LOG_LEVELS = {"error", "warn", "info", "debug"}

DEFAULT_SETTINGS: Dict[str, Any] = {
    "autostart": True,
    "resolution": "1920x1080x24",
    "max_fps": 60,
    "audio_enabled": True,
    "clipboard_sync": True,
    "gaming_chords": True,
    "composite_force": True,
    "always_active": False,
    "display": ":20",
    "log_level": "info",
}


def _is_root() -> bool:
    geteuid = getattr(os, "geteuid", None)
    return bool(geteuid and geteuid() == 0)


def _deck_user_info() -> Tuple[str, int, int, str, Dict[str, str]]:
    user = getattr(decky, "DECKY_USER", None) or os.environ.get("DECKY_USER") or "deck"
    if pwd is None:
        home = os.path.expanduser("~")
        return user, 1000, 1000, home, {"HOME": home, "USER": user, "PATH": os.environ.get("PATH", "")}
    try:
        pw = pwd.getpwnam(user)
    except KeyError:
        try:
            pw = pwd.getpwuid(1000)
            user = pw.pw_name
        except KeyError:
            pw = pwd.getpwuid(os.getuid())
            user = pw.pw_name

    uid = pw.pw_uid
    gid = pw.pw_gid
    home = pw.pw_dir

    env = os.environ.copy()
    for xdg_key in (
        "XDG_CONFIG_HOME",
        "XDG_DATA_HOME",
        "XDG_CACHE_HOME",
        "XDG_STATE_HOME",
    ):
        env.pop(xdg_key, None)

    deck_path = (
        f"{home}/.local/share/steamos-chromoting/bin:"
        f"{home}/.local/bin:"
        "/usr/local/bin:/usr/bin:/bin"
    )
    env["HOME"] = home
    env["USER"] = user
    env["LOGNAME"] = user
    env["XDG_RUNTIME_DIR"] = f"/run/user/{uid}"
    env["DBUS_SESSION_BUS_ADDRESS"] = f"unix:path=/run/user/{uid}/bus"
    env["PATH"] = deck_path
    return user, uid, gid, home, env


def _build_deck_cmd(args: List[str]) -> Tuple[List[str], Dict[str, str]]:
    user, uid, _, home, env = _deck_user_info()
    if _is_root():
        cmd = [
            "runuser",
            "-u",
            user,
            "--",
            "env",
            f"HOME={home}",
            f"USER={user}",
            f"LOGNAME={user}",
            f"XDG_RUNTIME_DIR=/run/user/{uid}",
            f"DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/{uid}/bus",
            f"PATH={env['PATH']}",
        ] + args
    else:
        cmd = args
    return cmd, env


def _run_as_deck(args: List[str], timeout: int = 20) -> subprocess.CompletedProcess:
    cmd, env = _build_deck_cmd(args)
    return subprocess.run(
        cmd,
        env=env,
        capture_output=True,
        text=True,
        timeout=timeout,
    )


def _makedirs_as_deck(path: str, mode: int = 0o755) -> None:
    _, uid, gid, _, _ = _deck_user_info()
    parts_to_fix: List[str] = []
    cur = os.path.abspath(path)
    while cur and not os.path.exists(cur):
        parts_to_fix.append(cur)
        parent = os.path.dirname(cur)
        if parent == cur:
            break
        cur = parent

    os.makedirs(path, mode=mode, exist_ok=True)
    for created in reversed(parts_to_fix):
        try:
            os.chmod(created, mode)
            if _is_root():
                os.chown(created, uid, gid)
        except OSError:
            pass
    try:
        os.chmod(path, mode)
        if _is_root():
            os.chown(path, uid, gid)
    except OSError:
        pass


def _write_file_as_deck(path: str, content: str, mode: int = 0o600, dir_mode: int = 0o755) -> None:
    _, uid, gid, _, _ = _deck_user_info()
    parent = os.path.dirname(os.path.abspath(path))
    _makedirs_as_deck(parent, mode=dir_mode)

    fd, tmp_path = tempfile.mkstemp(prefix=".tmp_crd_", dir=parent)
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="\n") as f:
            f.write(content)
        os.chmod(tmp_path, mode)
        if _is_root():
            os.chown(tmp_path, uid, gid)
        os.replace(tmp_path, path)
    finally:
        if os.path.exists(tmp_path):
            try:
                os.remove(tmp_path)
            except OSError:
                pass


def _proc_real_uid(proc_dir: str) -> Optional[int]:
    try:
        with open(os.path.join(proc_dir, "status"), "r", encoding="utf-8") as f:
            for line in f:
                if line.startswith("Uid:"):
                    parts = line.split()
                    if len(parts) >= 2:
                        return int(parts[1])
    except Exception:
        pass
    return None


def _collect_udp_socket_inodes() -> set[int]:
    inodes: set[int] = set()
    for net_path in ("/proc/net/udp", "/proc/net/udp6"):
        try:
            with open(net_path, "r", encoding="utf-8") as f:
                next(f, None)
                for line in f:
                    cols = line.split()
                    if len(cols) > 9:
                        inode = int(cols[9])
                        if inode != 0:
                            inodes.add(inode)
        except Exception:
            continue
    return inodes


def _detect_client_connected(uid: int) -> bool:
    udp_inodes = _collect_udp_socket_inodes()
    try:
        for pid in os.listdir("/proc"):
            if not pid.isdigit():
                continue
            proc_dir = f"/proc/{pid}"
            try:
                with open(f"{proc_dir}/comm", "r", encoding="utf-8") as cf:
                    comm = cf.read().strip()
                if comm not in ("chrome-remote-d", "remoting_me2me_"):
                    continue
                if _proc_real_uid(proc_dir) != uid:
                    continue
                fd_dir = f"{proc_dir}/fd"
                for fd in os.listdir(fd_dir):
                    try:
                        target = os.readlink(f"{fd_dir}/{fd}")
                    except OSError:
                        continue
                    if "audio.fifo" in target:
                        return True
                    if target.startswith("socket:[") and target.endswith("]"):
                        inode_str = target[8:-1]
                        if inode_str.isdigit() and int(inode_str) in udp_inodes:
                            return True
            except Exception:
                continue
    except Exception:
        pass
    return False


def _parse_env_file(path: str) -> Dict[str, str]:
    out: Dict[str, str] = {}
    if not os.path.isfile(path):
        return out
    try:
        with open(path, "r", encoding="utf-8") as f:
            for raw_line in f:
                line = raw_line.strip()
                if not line or line.startswith("#"):
                    continue
                if line.startswith("export "):
                    line = line[7:].strip()
                if "=" not in line:
                    continue
                k, v = line.split("=", 1)
                k = k.strip()
                v = v.strip().strip('"').strip("'")
                if k:
                    out[k] = v
    except Exception:
        pass
    return out


def _env_to_bool(val: Optional[str], default: bool) -> bool:
    if val is None:
        return default
    return val.strip().lower() not in ("0", "false", "no", "off")


def _parse_auth_input(raw: str) -> Tuple[Optional[str], str]:
    cleaned = raw.replace("\r", " ").replace("\n", " ").strip()
    if not cleaned:
        return None, DEFAULT_REDIRECT_URL

    code: Optional[str] = None
    for pattern in (
        r'--code="([^"]+)"',
        r"--code='([^']+)'",
        r'--code=([^\s"\']+)',
    ):
        m = re.search(pattern, cleaned)
        if m:
            code = m.group(1).strip()
            break

    if not code:
        candidate = cleaned.strip("\"' ")
        if (
            candidate
            and " " not in candidate
            and not candidate.startswith("http://")
            and not candidate.startswith("https://")
            and len(candidate) >= 10
        ):
            code = candidate

    redirect_url = DEFAULT_REDIRECT_URL
    for pattern in (
        r'--redirect-url="([^"]+)"',
        r"--redirect-url='([^']+)'",
        r'--redirect-url=([^\s"\']+)',
    ):
        m = re.search(pattern, cleaned)
        if m:
            redirect_url = m.group(1).strip()
            break

    return code, redirect_url


def _make_host_pin_hash(host_id: str, pin: str) -> str:
    digest = hmac.new(
        host_id.encode("utf-8"),
        pin.encode("utf-8"),
        hashlib.sha256,
    ).digest()
    return "hmac:" + base64.b64encode(digest).decode("ascii")


def _validate_and_normalize_settings(
    incoming: Dict[str, Any],
    base: Dict[str, Any],
) -> Dict[str, Any]:
    merged = dict(base)

    if "autostart" in incoming:
        merged["autostart"] = bool(incoming["autostart"])

    if "resolution" in incoming:
        res = str(incoming["resolution"]).strip()
        if res in VALID_RESOLUTIONS or re.match(r"^\d{3,4}x\d{3,4}x(16|24|32)$", res):
            merged["resolution"] = res

    if "max_fps" in incoming:
        try:
            fps = int(incoming["max_fps"])
            if fps in VALID_FPS:
                merged["max_fps"] = fps
            else:
                merged["max_fps"] = max(10, min(120, fps))
        except (TypeError, ValueError):
            pass

    for bool_key in (
        "audio_enabled",
        "clipboard_sync",
        "gaming_chords",
        "composite_force",
        "always_active",
    ):
        if bool_key in incoming:
            merged[bool_key] = bool(incoming[bool_key])

    if "display" in incoming:
        disp = str(incoming["display"]).strip()
        if re.match(r"^:\d{1,3}$", disp):
            merged["display"] = disp

    if "log_level" in incoming:
        lvl = str(incoming["log_level"]).strip().lower()
        if lvl in VALID_LOG_LEVELS:
            merged["log_level"] = lvl

    return merged


def _format_settings_env(settings: Dict[str, Any]) -> str:
    lines = [
        "# Managed by SteamOS Chromoting",
        f"STEAMOS_CRD_DISPLAY={settings['display']}",
        f"STEAMOS_CRD_RESOLUTION={settings['resolution']}",
        f"STEAMOS_CRD_MAX_FPS={int(settings['max_fps'])}",
        f"STEAMOS_CRD_AUDIO_ENABLED={'1' if settings['audio_enabled'] else '0'}",
        f"STEAMOS_CRD_CLIPBOARD_SYNC={'1' if settings['clipboard_sync'] else '0'}",
        f"STEAMOS_CRD_GAMING_CHORDS={'1' if settings['gaming_chords'] else '0'}",
        f"STEAMOS_CRD_COMPOSITE_FORCE={'1' if settings['composite_force'] else '0'}",
        f"STEAMOS_CRD_ALWAYS_ACTIVE={'1' if settings['always_active'] else '0'}",
        f"RUST_LOG={settings['log_level']}",
    ]
    return "\n".join(lines) + "\n"


class Plugin:
    def __init__(self) -> None:
        self._install_lock = asyncio.Lock()
        self._install_task: Optional[asyncio.Task] = None
        self._install_state: str = "idle"
        self._install_step: str = ""
        self._install_percent: int = 0
        self._install_error: Optional[str] = None
        self._install_log: List[str] = []

    async def _main(self) -> None:
        decky.logger.info("SteamOS Chromoting Decky plugin loaded")

    async def _unload(self) -> None:
        if self._install_task and not self._install_task.done():
            self._install_task.cancel()
        decky.logger.info("SteamOS Chromoting Decky plugin unloaded")

    def _paths(self) -> Dict[str, str]:
        _, _, _, home, _ = _deck_user_info()
        install_dir = os.path.join(home, ".local", "share", "steamos-chromoting")
        return {
            "home": home,
            "install_dir": install_dir,
            "cli": os.path.join(install_dir, "bin", "steamos-crd"),
            "bridge": os.path.join(install_dir, "bin", "steamos-crd-bridge"),
            "host_runner": os.path.join(install_dir, "runtime", "host-runner.sh"),
            "crd_opt": os.path.join(install_dir, "opt", "google", "chrome-remote-desktop"),
            "version_file": os.path.join(install_dir, "VERSION"),
            "crd_cfg_dir": os.path.join(home, ".config", "chrome-remote-desktop"),
            "settings_dir": os.path.join(home, ".config", "steamos-chromoting"),
            "settings_file": os.path.join(home, ".config", "steamos-chromoting", "settings.env"),
            "systemd_dir": os.path.join(home, ".config", "systemd", "user"),
        }

    def _read_settings_sync(self, autostart_override: Optional[bool] = None) -> Dict[str, Any]:
        paths = self._paths()
        raw = _parse_env_file(paths["settings_file"])

        autostart = DEFAULT_SETTINGS["autostart"]
        if autostart_override is not None:
            autostart = autostart_override
        else:
            target_unit = os.path.join(paths["systemd_dir"], "steamos-crd.target")
            if os.path.isfile(target_unit):
                try:
                    res = _run_as_deck(
                        ["systemctl", "--user", "is-enabled", "steamos-crd.target"],
                        timeout=5,
                    )
                    autostart = res.stdout.strip() == "enabled"
                except Exception:
                    pass

        fps_val = DEFAULT_SETTINGS["max_fps"]
        if "STEAMOS_CRD_MAX_FPS" in raw:
            try:
                fps_val = max(10, min(120, int(raw["STEAMOS_CRD_MAX_FPS"])))
            except ValueError:
                fps_val = DEFAULT_SETTINGS["max_fps"]

        resolution = raw.get("STEAMOS_CRD_RESOLUTION", DEFAULT_SETTINGS["resolution"])
        if not re.match(r"^\d{3,4}x\d{3,4}x(16|24|32)$", resolution):
            resolution = DEFAULT_SETTINGS["resolution"]

        display = raw.get("STEAMOS_CRD_DISPLAY", DEFAULT_SETTINGS["display"])
        if not re.match(r"^:\d{1,3}$", display):
            display = DEFAULT_SETTINGS["display"]

        log_level = raw.get("RUST_LOG", DEFAULT_SETTINGS["log_level"]).lower()
        if log_level not in VALID_LOG_LEVELS:
            log_level = DEFAULT_SETTINGS["log_level"]

        return {
            "autostart": autostart,
            "resolution": resolution,
            "max_fps": fps_val,
            "audio_enabled": _env_to_bool(raw.get("STEAMOS_CRD_AUDIO_ENABLED"), True),
            "clipboard_sync": _env_to_bool(raw.get("STEAMOS_CRD_CLIPBOARD_SYNC"), True),
            "gaming_chords": _env_to_bool(raw.get("STEAMOS_CRD_GAMING_CHORDS"), True),
            "composite_force": _env_to_bool(raw.get("STEAMOS_CRD_COMPOSITE_FORCE"), True),
            "always_active": _env_to_bool(raw.get("STEAMOS_CRD_ALWAYS_ACTIVE"), False),
            "display": display,
            "log_level": log_level,
        }

    def _ensure_systemd_dropins_sync(self, settings: Dict[str, Any]) -> None:
        paths = self._paths()
        systemd_dir = paths["systemd_dir"]
        if not os.path.isdir(systemd_dir):
            return

        for svc in (
            "steamos-crd-xvfb.service",
            "steamos-crd-audio.service",
            "steamos-crd-bridge.service",
            "steamos-crd-host.service",
        ):
            dropin_dir = os.path.join(systemd_dir, f"{svc}.d")
            lines = [
                "[Service]",
                "EnvironmentFile=-%h/.config/steamos-chromoting/settings.env",
            ]
            if svc == "steamos-crd-host.service" and not settings.get("audio_enabled", True):
                lines.append("Environment=CHROME_REMOTE_DESKTOP_AUDIO_PIPE=")
            content = "\n".join(lines) + "\n"
            _write_file_as_deck(
                os.path.join(dropin_dir, "override.conf"),
                content,
                mode=0o644,
                dir_mode=0o755,
            )

    def _find_host_config_sync(self) -> Tuple[Optional[str], Dict[str, Any]]:
        paths = self._paths()
        configs = sorted(glob.glob(os.path.join(paths["crd_cfg_dir"], "host#*.json")))
        if not configs:
            return None, {}
        cfg_path = configs[0]
        try:
            with open(cfg_path, "r", encoding="utf-8") as f:
                data = json.load(f)
                if isinstance(data, dict):
                    return cfg_path, data
        except Exception as e:
            decky.logger.warning(f"Failed reading CRD host config {cfg_path}: {e}")
        return cfg_path, {}

    def _check_installed_sync(self) -> Tuple[bool, bool, bool, Optional[str]]:
        paths = self._paths()
        cli_ok = os.path.isfile(paths["cli"]) and os.access(paths["cli"], os.X_OK)
        bridge_ok = os.path.isfile(paths["bridge"]) and os.access(paths["bridge"], os.X_OK)
        runner_ok = os.path.isfile(paths["host_runner"])
        crd_bin_1 = os.path.join(paths["crd_opt"], "chrome-remote-desktop-host")
        crd_bin_2 = os.path.join(paths["crd_opt"], "remoting_me2me_host")
        crd_ok = (os.path.isfile(crd_bin_1) and os.access(crd_bin_1, os.X_OK)) or (
            os.path.isfile(crd_bin_2) and os.access(crd_bin_2, os.X_OK)
        )

        version: Optional[str] = None
        if os.path.isfile(paths["version_file"]):
            try:
                with open(paths["version_file"], "r", encoding="utf-8") as vf:
                    version = vf.read().strip() or None
            except Exception:
                pass

        installed = cli_ok and bridge_ok and runner_ok and crd_ok
        return installed, crd_ok, bridge_ok, version

    def _get_status_sync(self) -> Dict[str, Any]:
        _, uid, _, _, _ = _deck_user_info()
        installed, crd_ok, bridge_ok, version = self._check_installed_sync()
        cfg_path, cfg_data = self._find_host_config_sync()
        registered = cfg_path is not None

        host_name = cfg_data.get("host_name") or (socket.gethostname() if not registered else "Registered Host")
        host_owner = cfg_data.get("host_owner") or cfg_data.get("host_owner_email") or None
        host_id = cfg_data.get("host_id") or None

        host_active = False
        bridge_active = False
        xvfb_active = False
        audio_active = False
        autostart = DEFAULT_SETTINGS["autostart"]

        if installed:
            try:
                res = _run_as_deck(
                    [
                        "systemctl",
                        "--user",
                        "is-active",
                        "steamos-crd-host.service",
                        "steamos-crd-bridge.service",
                        "steamos-crd-xvfb.service",
                        "steamos-crd-audio.service",
                    ],
                    timeout=8,
                )
                states = [line.strip() for line in res.stdout.splitlines()]
                while len(states) < 4:
                    states.append("inactive")
                host_active = states[0] == "active"
                bridge_active = states[1] == "active"
                xvfb_active = states[2] == "active"
                audio_active = states[3] == "active"
            except Exception as e:
                decky.logger.warning(f"systemctl is-active check failed: {e}")

            try:
                en_res = _run_as_deck(
                    ["systemctl", "--user", "is-enabled", "steamos-crd.target"],
                    timeout=5,
                )
                autostart = en_res.stdout.strip() == "enabled"
            except Exception:
                pass

        uinput_writable = False
        try:
            u_res = _run_as_deck(["test", "-w", "/dev/uinput"], timeout=5)
            uinput_writable = u_res.returncode == 0
        except Exception:
            pass

        settings = self._read_settings_sync(autostart_override=autostart)
        client_connected = _detect_client_connected(uid) if host_active else False

        return {
            "installed": installed,
            "crd_deb_installed": crd_ok,
            "bridge_installed": bridge_ok,
            "server_version": version,
            "registered": registered,
            "host_name": host_name,
            "host_owner": host_owner,
            "host_id": host_id,
            "default_hostname": socket.gethostname() or "steamdeck",
            "enabled": host_active and bridge_active,
            "host_active": host_active,
            "bridge_active": bridge_active,
            "xvfb_active": xvfb_active,
            "audio_active": audio_active,
            "client_connected": client_connected,
            "uinput_writable": uinput_writable,
            "install_state": self._install_state,
            "install_step": self._install_step,
            "install_percent": self._install_percent,
            "install_error": self._install_error,
            "install_log": list(self._install_log[-40:]),
            "settings": settings,
        }

    async def _emit_progress(
        self,
        state: str,
        step: str,
        percent: int,
        error: Optional[str] = None,
        log_line: Optional[str] = None,
    ) -> None:
        self._install_state = state
        self._install_step = step
        self._install_percent = percent
        self._install_error = error
        if log_line:
            clean = log_line.strip()
            if clean:
                self._install_log.append(clean)
                if len(self._install_log) > 100:
                    self._install_log = self._install_log[-100:]
        try:
            await decky.emit(
                "crd_install_progress",
                {
                    "state": self._install_state,
                    "step": self._install_step,
                    "percent": self._install_percent,
                    "error": self._install_error,
                    "log": list(self._install_log[-40:]),
                },
            )
        except Exception:
            pass

    async def get_status(self) -> Dict[str, Any]:
        return await asyncio.to_thread(self._get_status_sync)

    async def set_enabled(self, enabled: bool) -> Dict[str, Any]:
        def _toggle() -> Dict[str, Any]:
            if enabled:
                settings = self._read_settings_sync()
                self._ensure_systemd_dropins_sync(settings)
                _run_as_deck(["systemctl", "--user", "daemon-reload"], timeout=10)
                _run_as_deck(["systemctl", "--user", "start", "steamos-crd.target"], timeout=15)
                _run_as_deck(
                    [
                        "systemctl",
                        "--user",
                        "start",
                        "steamos-crd-xvfb.service",
                        "steamos-crd-audio.service",
                        "steamos-crd-bridge.service",
                        "steamos-crd-host.service",
                    ],
                    timeout=15,
                )
            else:
                _run_as_deck(["systemctl", "--user", "stop", "steamos-crd.target"], timeout=15)
                _run_as_deck(
                    [
                        "systemctl",
                        "--user",
                        "stop",
                        "steamos-crd-host.service",
                        "steamos-crd-bridge.service",
                        "steamos-crd-audio.service",
                        "steamos-crd-xvfb.service",
                    ],
                    timeout=15,
                )
            return self._get_status_sync()

        return await asyncio.to_thread(_toggle)

    async def restart_service(self) -> Dict[str, Any]:
        def _restart() -> Dict[str, Any]:
            paths = self._paths()
            settings = self._read_settings_sync()
            self._ensure_systemd_dropins_sync(settings)
            _run_as_deck(["systemctl", "--user", "daemon-reload"], timeout=10)
            if os.path.isfile(paths["cli"]):
                _run_as_deck([paths["cli"], "restart"], timeout=20)
            else:
                _run_as_deck(
                    [
                        "systemctl",
                        "--user",
                        "restart",
                        "steamos-crd-xvfb.service",
                        "steamos-crd-audio.service",
                        "steamos-crd-host.service",
                        "steamos-crd-bridge.service",
                    ],
                    timeout=20,
                )
            return self._get_status_sync()

        return await asyncio.to_thread(_restart)

    async def get_settings(self) -> Dict[str, Any]:
        return await asyncio.to_thread(self._read_settings_sync)

    async def save_settings(self, new_settings: Dict[str, Any]) -> Dict[str, Any]:
        def _save() -> Dict[str, Any]:
            paths = self._paths()
            old = self._read_settings_sync()
            updated = _validate_and_normalize_settings(new_settings or {}, old)

            _write_file_as_deck(
                paths["settings_file"],
                _format_settings_env(updated),
                mode=0o600,
                dir_mode=0o700,
            )

            installed, _, _, _ = self._check_installed_sync()
            if installed:
                self._ensure_systemd_dropins_sync(updated)
                _run_as_deck(["systemctl", "--user", "daemon-reload"], timeout=10)

                if updated["autostart"] != old["autostart"]:
                    action = "enable" if updated["autostart"] else "disable"
                    _run_as_deck(
                        ["systemctl", "--user", action, "steamos-crd.target"],
                        timeout=10,
                    )

                status_now = self._get_status_sync()
                any_running = (
                    status_now["host_active"]
                    or status_now["bridge_active"]
                    or status_now["xvfb_active"]
                )

                if any_running:
                    need_full_restart = (
                        updated["resolution"] != old["resolution"]
                        or updated["display"] != old["display"]
                    )
                    need_audio_host_restart = updated["audio_enabled"] != old["audio_enabled"]
                    need_bridge_restart = any(
                        updated[k] != old[k]
                        for k in (
                            "max_fps",
                            "clipboard_sync",
                            "gaming_chords",
                            "composite_force",
                            "always_active",
                            "log_level",
                        )
                    )

                    if need_full_restart:
                        _run_as_deck(
                            [
                                "systemctl",
                                "--user",
                                "restart",
                                "steamos-crd-xvfb.service",
                                "steamos-crd-audio.service",
                                "steamos-crd-host.service",
                                "steamos-crd-bridge.service",
                            ],
                            timeout=20,
                        )
                    else:
                        if need_audio_host_restart:
                            _run_as_deck(
                                [
                                    "systemctl",
                                    "--user",
                                    "restart",
                                    "steamos-crd-audio.service",
                                    "steamos-crd-host.service",
                                ],
                                timeout=15,
                            )
                        if need_bridge_restart:
                            _run_as_deck(
                                [
                                    "systemctl",
                                    "--user",
                                    "restart",
                                    "steamos-crd-bridge.service",
                                ],
                                timeout=15,
                            )

            return self._get_status_sync()

        return await asyncio.to_thread(_save)

    async def reset_settings(self) -> Dict[str, Any]:
        return await self.save_settings(dict(DEFAULT_SETTINGS))

    async def install_server(self) -> Dict[str, Any]:
        return await self._start_install_job(is_update=False)

    async def update_server(self) -> Dict[str, Any]:
        return await self._start_install_job(is_update=True)

    async def _start_install_job(self, is_update: bool) -> Dict[str, Any]:
        if self._install_lock.locked():
            return await self.get_status()

        self._install_log = []
        label = "Updating" if is_update else "Installing"
        await self._emit_progress(
            "updating" if is_update else "installing",
            f"{label} SteamOS Chromoting server...",
            5,
            error=None,
        )
        self._install_task = asyncio.create_task(self._perform_install(is_update))
        return await self.get_status()

    async def _perform_install(self, is_update: bool) -> None:
        async with self._install_lock:
            user, uid, gid, _, _ = _deck_user_info()
            state_name = "updating" if is_update else "installing"
            tmp_dir = tempfile.mkdtemp(prefix="steamos-crd-decky-")
            try:
                os.chmod(tmp_dir, 0o755)
                if _is_root():
                    os.chown(tmp_dir, uid, gid)

                tarball_path = os.path.join(tmp_dir, "steamos-chromoting.tar.gz")
                custom_tarball = os.environ.get("STEAMOS_CRD_RELEASE_TARBALL", "").strip()

                if custom_tarball and os.path.isfile(custom_tarball):
                    await self._emit_progress(
                        state_name,
                        "Copying local release archive...",
                        15,
                        log_line=f"Using local archive: {custom_tarball}",
                    )
                    await asyncio.to_thread(shutil.copyfile, custom_tarball, tarball_path)
                else:
                    await self._emit_progress(
                        state_name,
                        "Downloading latest release from GitHub...",
                        12,
                        log_line=f"Downloading {RELEASE_TARBALL_URL}",
                    )

                    def _download() -> None:
                        curl_bin = shutil.which("curl")
                        if curl_bin:
                            res = subprocess.run(
                                [
                                    curl_bin,
                                    "-fL",
                                    "--retry",
                                    "3",
                                    "--connect-timeout",
                                    "20",
                                    "-o",
                                    tarball_path,
                                    RELEASE_TARBALL_URL,
                                ],
                                capture_output=True,
                                text=True,
                                timeout=180,
                            )
                            if res.returncode != 0:
                                raise RuntimeError(
                                    res.stderr.strip() or f"curl exited with {res.returncode}"
                                )
                        else:
                            req = urllib.request.Request(
                                RELEASE_TARBALL_URL,
                                headers={"User-Agent": "SteamOS-Chromoting-Decky"},
                            )
                            with urllib.request.urlopen(req, timeout=180) as resp, open(
                                tarball_path, "wb"
                            ) as out_f:
                                shutil.copyfileobj(resp, out_f)

                        os.chmod(tarball_path, 0o644)
                        if _is_root():
                            os.chown(tarball_path, uid, gid)

                    await asyncio.to_thread(_download)

                await self._emit_progress(
                    state_name,
                    "Extracting server package...",
                    30,
                    log_line="Extracting steamos-chromoting-linux-x86_64.tar.gz",
                )

                def _extract() -> str:
                    res = _run_as_deck(
                        ["tar", "-xzf", tarball_path, "-C", tmp_dir],
                        timeout=60,
                    )
                    if res.returncode != 0:
                        raise RuntimeError(res.stderr.strip() or "Failed to extract release archive")

                    pkg_dir = os.path.join(tmp_dir, "steamos-chromoting")
                    if not os.path.isdir(pkg_dir):
                        raise RuntimeError("Extracted archive missing 'steamos-chromoting/' folder")

                    for rel in ("cli/steamos-crd", "dist/steamos-crd-bridge"):
                        full = os.path.join(pkg_dir, rel)
                        if os.path.isfile(full):
                            os.chmod(full, 0o755)
                    for sh_file in glob.glob(os.path.join(pkg_dir, "runtime", "*.sh")):
                        os.chmod(sh_file, 0o755)
                    return pkg_dir

                pkg_dir = await asyncio.to_thread(_extract)
                cli_installer = os.path.join(pkg_dir, "cli", "steamos-crd")

                await self._emit_progress(
                    state_name,
                    "Installing runtime & official Google CRD host...",
                    45,
                    log_line="Running steamos-crd install...",
                )

                cmd, env = _build_deck_cmd([cli_installer, "install"])
                proc = await asyncio.create_subprocess_exec(
                    *cmd,
                    env=env,
                    stdout=asyncio.subprocess.PIPE,
                    stderr=asyncio.subprocess.STDOUT,
                )

                current_pct = 45
                assert proc.stdout is not None
                try:
                    async with asyncio.timeout(600):
                        while True:
                            raw_line = await proc.stdout.readline()
                            if not raw_line:
                                break
                            line = raw_line.decode("utf-8", errors="replace").strip()
                            if not line:
                                continue
                            step_msg = self._install_step
                            if "Downloading official Chrome Remote Desktop" in line:
                                current_pct = 58
                                step_msg = "Downloading official Google CRD host (.deb)..."
                            elif "Extracting Chrome Remote Desktop" in line:
                                current_pct = 72
                                step_msg = "Extracting Chrome Remote Desktop binaries..."
                            elif "Extracting Xvfb" in line or "Bundling Xvfb" in line:
                                current_pct = 82
                                step_msg = "Bundling Xvfb sysroot via Podman (first-time setup)..."
                            elif "Installation complete" in line:
                                current_pct = 92
                                step_msg = "Finalizing systemd user services..."
                            await self._emit_progress(
                                state_name,
                                step_msg,
                                current_pct,
                                log_line=line,
                            )
                        rc = await proc.wait()
                except TimeoutError as exc:
                    proc.kill()
                    raise RuntimeError("Server installation timed out after 10 minutes") from exc

                if rc != 0:
                    tail = "\n".join(self._install_log[-8:])
                    raise RuntimeError(f"Installer exited with code {rc}.\n{tail}")

                await self._emit_progress(
                    state_name,
                    "Applying user linger & saved settings...",
                    96,
                )

                def _finalize() -> None:
                    if _is_root():
                        subprocess.run(
                            ["loginctl", "enable-linger", user],
                            capture_output=True,
                            timeout=10,
                        )
                    paths = self._paths()
                    settings = self._read_settings_sync()
                    if not os.path.isfile(paths["settings_file"]):
                        _write_file_as_deck(
                            paths["settings_file"],
                            _format_settings_env(settings),
                            mode=0o600,
                            dir_mode=0o700,
                        )
                    self._ensure_systemd_dropins_sync(settings)
                    _run_as_deck(["systemctl", "--user", "daemon-reload"], timeout=10)

                    cfg_path, _ = self._find_host_config_sync()
                    if cfg_path and os.path.isfile(paths["cli"]):
                        _run_as_deck([paths["cli"], "restart"], timeout=20)

                await asyncio.to_thread(_finalize)
                await self._emit_progress(
                    "idle",
                    "Server installed successfully!",
                    100,
                    error=None,
                    log_line="SteamOS Chromoting installation complete.",
                )
            except asyncio.CancelledError:
                await self._emit_progress("idle", "Cancelled", 0, error=None)
                raise
            except Exception as e:
                decky.logger.error(f"Server install/update failed: {e}")
                await self._emit_progress(
                    "error",
                    "Installation failed",
                    self._install_percent,
                    error=str(e),
                    log_line=f"ERROR: {e}",
                )
            finally:
                try:
                    shutil.rmtree(tmp_dir, ignore_errors=True)
                except Exception:
                    pass

    async def uninstall_server(self, purge: bool = False) -> Dict[str, Any]:
        async with self._install_lock:
            await self._emit_progress(
                "uninstalling",
                "Uninstalling SteamOS Chromoting server...",
                30,
            )

            def _do_uninstall() -> Dict[str, Any]:
                paths = self._paths()
                if os.path.isfile(paths["cli"]):
                    args = [paths["cli"], "uninstall"]
                    if purge:
                        args.append("--purge")
                    _run_as_deck(args, timeout=30)
                else:
                    _run_as_deck(
                        [
                            "systemctl",
                            "--user",
                            "disable",
                            "--now",
                            "steamos-crd.target",
                            "steamos-crd-xvfb.service",
                            "steamos-crd-audio.service",
                            "steamos-crd-bridge.service",
                            "steamos-crd-host.service",
                        ],
                        timeout=15,
                    )
                    shutil.rmtree(paths["install_dir"], ignore_errors=True)

                for pattern in ("steamos-crd*",):
                    for item in glob.glob(os.path.join(paths["systemd_dir"], pattern)):
                        if os.path.isdir(item):
                            shutil.rmtree(item, ignore_errors=True)
                        else:
                            try:
                                os.remove(item)
                            except OSError:
                                pass
                _run_as_deck(["systemctl", "--user", "daemon-reload"], timeout=10)

                if purge:
                    shutil.rmtree(paths["crd_cfg_dir"], ignore_errors=True)
                    shutil.rmtree(paths["settings_dir"], ignore_errors=True)

                return self._get_status_sync()

            try:
                status = await asyncio.to_thread(_do_uninstall)
                await self._emit_progress("idle", "", 0, error=None)
                status["install_state"] = "idle"
                status["install_step"] = ""
                status["install_percent"] = 0
                return status
            except Exception as e:
                await self._emit_progress("error", "Uninstall failed", 0, error=str(e))
                return await self.get_status()

    async def setup_host(
        self,
        raw_code: str,
        pin: str,
        host_name: str,
    ) -> Dict[str, Any]:
        code, redirect_url = _parse_auth_input(raw_code or "")
        if not code:
            return {
                "ok": False,
                "error": (
                    "Invalid authorization input. Open Google's Headless setup page, "
                    "authorize, and paste the copied 'Debian Linux' command."
                ),
                "status": await self.get_status(),
            }

        clean_pin = (pin or "").strip()
        if not re.match(r"^\d{6,}$", clean_pin):
            return {
                "ok": False,
                "error": "PIN must be at least 6 digits (0–9 only).",
                "status": await self.get_status(),
            }

        clean_name = (host_name or "").strip() or socket.gethostname() or "steamdeck"
        clean_name = re.sub(r"[\r\n\t]", " ", clean_name)[:64].strip()

        def _do_setup() -> Tuple[bool, Optional[str], Dict[str, Any]]:
            user, _, _, _, _ = _deck_user_info()
            paths = self._paths()
            if not os.path.isfile(paths["cli"]):
                return (
                    False,
                    "Server runtime is not installed yet. Click 'Install Server' first.",
                    self._get_status_sync(),
                )

            if _is_root():
                subprocess.run(
                    ["loginctl", "enable-linger", user],
                    capture_output=True,
                    timeout=10,
                )

            settings = self._read_settings_sync()
            self._ensure_systemd_dropins_sync(settings)

            res = _run_as_deck(
                [
                    paths["cli"],
                    "setup",
                    f"--code={code}",
                    f"--pin={clean_pin}",
                    f"--name={clean_name}",
                    f"--redirect-url={redirect_url}",
                ],
                timeout=60,
            )
            if res.returncode != 0:
                combined = ((res.stdout or "") + "\n" + (res.stderr or "")).strip()
                tail = "\n".join(combined.splitlines()[-8:]) or "Host registration failed."
                return False, tail, self._get_status_sync()

            return True, None, self._get_status_sync()

        ok, err, status = await asyncio.to_thread(_do_setup)
        return {"ok": ok, "error": err, "status": status}

    async def change_pin(self, new_pin: str) -> Dict[str, Any]:
        clean_pin = (new_pin or "").strip()
        if not re.match(r"^\d{6,}$", clean_pin):
            return {
                "ok": False,
                "error": "PIN must be at least 6 digits (0–9 only).",
                "status": await self.get_status(),
            }

        def _update_pin() -> Tuple[bool, Optional[str], Dict[str, Any]]:
            cfg_path, data = self._find_host_config_sync()
            if not cfg_path or not data:
                return (
                    False,
                    "Host is not registered yet. Pair your Google account first.",
                    self._get_status_sync(),
                )
            host_id = data.get("host_id")
            if not host_id or not isinstance(host_id, str):
                return (
                    False,
                    "Saved host configuration is missing 'host_id'. Re-pair Google account.",
                    self._get_status_sync(),
                )

            pin_hash = _make_host_pin_hash(host_id, clean_pin)
            data["host_secret_hash"] = pin_hash
            if "pin_hash" in data:
                data["pin_hash"] = pin_hash

            _write_file_as_deck(
                cfg_path,
                json.dumps(data, indent=2) + "\n",
                mode=0o600,
                dir_mode=0o700,
            )

            status_before = self._get_status_sync()
            if status_before["host_active"]:
                _run_as_deck(
                    ["systemctl", "--user", "restart", "steamos-crd-host.service"],
                    timeout=15,
                )
            return True, None, self._get_status_sync()

        ok, err, status = await asyncio.to_thread(_update_pin)
        return {"ok": ok, "error": err, "status": status}

    async def change_host_name(self, new_name: str) -> Dict[str, Any]:
        clean_name = re.sub(r"[\r\n\t]", " ", (new_name or "").strip())[:64].strip()
        if not clean_name:
            return {
                "ok": False,
                "error": "Device name cannot be empty.",
                "status": await self.get_status(),
            }

        def _update_name() -> Tuple[bool, Optional[str], Dict[str, Any]]:
            cfg_path, data = self._find_host_config_sync()
            if not cfg_path or not data:
                return (
                    False,
                    "Host is not registered yet. Pair your Google account first.",
                    self._get_status_sync(),
                )

            data["host_name"] = clean_name
            _write_file_as_deck(
                cfg_path,
                json.dumps(data, indent=2) + "\n",
                mode=0o600,
                dir_mode=0o700,
            )

            status_before = self._get_status_sync()
            if status_before["host_active"]:
                _run_as_deck(
                    ["systemctl", "--user", "restart", "steamos-crd-host.service"],
                    timeout=15,
                )
            return True, None, self._get_status_sync()

        ok, err, status = await asyncio.to_thread(_update_name)
        return {"ok": ok, "error": err, "status": status}

    async def get_clipboard_text(self) -> Dict[str, str]:
        def _read_clip() -> str:
            probes: List[List[str]] = [
                ["wl-paste", "--no-newline"],
                ["env", "DISPLAY=:0", "xclip", "-selection", "clipboard", "-o"],
                ["env", "DISPLAY=:1", "xclip", "-selection", "clipboard", "-o"],
                ["env", "DISPLAY=:0", "xsel", "--clipboard", "--output"],
                [
                    "dbus-send",
                    "--session",
                    "--print-reply=literal",
                    "--dest=org.kde.klipper",
                    "/klipper",
                    "org.kde.klipper.klipper.getClipboardContents",
                ],
            ]
            for cmd_args in probes:
                try:
                    res = _run_as_deck(cmd_args, timeout=3)
                    if res.returncode == 0 and res.stdout.strip():
                        return res.stdout.strip()
                except Exception:
                    continue
            return ""

        text = await asyncio.to_thread(_read_clip)
        return {"text": text}

    async def get_logs(self, unit: str = "all", lines: int = 80) -> Dict[str, str]:
        safe_lines = max(10, min(300, int(lines)))
        unit_map = {
            "host": "steamos-crd-host.service",
            "bridge": "steamos-crd-bridge.service",
            "xvfb": "steamos-crd-xvfb.service",
            "audio": "steamos-crd-audio.service",
        }
        unit_pattern = unit_map.get(unit, "steamos-crd-*.service")

        def _fetch_logs() -> str:
            try:
                res = _run_as_deck(
                    [
                        "journalctl",
                        "--user",
                        "-u",
                        unit_pattern,
                        "--no-pager",
                        "-n",
                        str(safe_lines),
                    ],
                    timeout=10,
                )
                out = (res.stdout or "").strip()
                if not out:
                    return "No journal entries recorded yet for this unit."
                return out
            except Exception as e:
                return f"Failed to read user journal logs: {e}"

        logs = await asyncio.to_thread(_fetch_logs)
        return {"logs": logs}
