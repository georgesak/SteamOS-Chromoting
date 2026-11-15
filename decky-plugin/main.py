import asyncio
import glob
import json
import os
import pwd
import subprocess
from typing import Dict, Any

import decky


def _deck_env() -> tuple[str, int, dict]:
    user = os.environ.get("DECKY_USER", "deck")
    pw = pwd.getpwnam(user)
    env = os.environ.copy()
    env["HOME"] = pw.pw_dir
    env["USER"] = user
    env["LOGNAME"] = user
    env["XDG_RUNTIME_DIR"] = f"/run/user/{pw.pw_uid}"
    env["DBUS_SESSION_BUS_ADDRESS"] = f"unix:path=/run/user/{pw.pw_uid}/bus"
    return user, pw.pw_uid, env


def _run_as_deck(args: list[str]) -> subprocess.CompletedProcess:
    user, uid, env = _deck_env()
    if os.geteuid() == 0:
        cmd = ["runuser", "-u", user, "--"] + args
    else:
        cmd = args
    return subprocess.run(cmd, env=env, capture_output=True, text=True, timeout=15)


class Plugin:
    async def _main(self):
        decky.logger.info("SteamOS Chromoting Decky plugin loaded")

    async def _unload(self):
        decky.logger.info("SteamOS Chromoting Decky plugin unloaded")

    async def get_status(self) -> Dict[str, Any]:
        _, uid, env = _deck_env()
        home = env["HOME"]
        cfg_dir = os.path.join(home, ".config", "chrome-remote-desktop")
        configs = glob.glob(os.path.join(cfg_dir, "host#*.json"))
        host_name = None
        if configs:
            try:
                with open(configs[0], "r", encoding="utf-8") as f:
                    data = json.load(f)
                    host_name = data.get("host_name")
            except Exception:
                pass

        host_res = await asyncio.to_thread(
            _run_as_deck, ["systemctl", "--user", "is-active", "steamos-crd-host.service"]
        )
        bridge_res = await asyncio.to_thread(
            _run_as_deck, ["systemctl", "--user", "is-active", "steamos-crd-bridge.service"]
        )

        host_active = host_res.stdout.strip() == "active"
        bridge_active = bridge_res.stdout.strip() == "active"

        # Check if a remote client currently has audio.fifo open
        client_connected = False
        try:
            for pid in os.listdir("/proc"):
                if not pid.isdigit():
                    continue
                fd_dir = f"/proc/{pid}/fd"
                try:
                    if os.stat(f"/proc/{pid}").st_uid != uid:
                        continue
                    with open(f"/proc/{pid}/comm", "r") as cf:
                        comm = cf.read().strip()
                    if comm not in ("chrome-remote-d", "remoting_me2me_"):
                        continue
                    for fd in os.listdir(fd_dir):
                        target = os.readlink(f"{fd_dir}/{fd}")
                        if "audio.fifo" in target:
                            client_connected = True
                            break
                except Exception:
                    continue
        except Exception:
            pass

        return {
            "registered": len(configs) > 0,
            "host_name": host_name or "Unregistered",
            "enabled": host_active and bridge_active,
            "host_active": host_active,
            "bridge_active": bridge_active,
            "client_connected": client_connected,
        }

    async def set_enabled(self, enabled: bool) -> Dict[str, Any]:
        action = "start" if enabled else "stop"
        await asyncio.to_thread(
            _run_as_deck, ["systemctl", "--user", action, "steamos-crd.target"]
        )
        return await self.get_status()

    async def restart_service(self) -> Dict[str, Any]:
        _, _, env = _deck_env()
        cli = os.path.join(env["HOME"], ".local/share/steamos-chromoting/bin/steamos-crd")
        await asyncio.to_thread(_run_as_deck, [cli, "restart"])
        return await self.get_status()
