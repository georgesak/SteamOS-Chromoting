use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use tracing::debug;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode};
use x11rb::wrapper::ConnectionExt as _;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionMode {
    GamingMode,
    DesktopX11 {
        display: String,
        xauthority: Option<String>,
    },
    DesktopWayland {
        dbus_addr: Option<String>,
    },
    Transitioning,
}

impl SessionMode {
    pub fn label(&self) -> &'static str {
        match self {
            SessionMode::GamingMode => "Gaming Mode (Gamescope)",
            SessionMode::DesktopX11 { .. } => "Desktop Mode (KDE Plasma X11)",
            SessionMode::DesktopWayland { .. } => "Desktop Mode (KDE Plasma Wayland)",
            SessionMode::Transitioning => "Switching SteamOS Session Mode...",
        }
    }
}

fn read_proc_environ(pid: u32) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let path = format!("/proc/{pid}/environ");
    if let Ok(bytes) = fs::read(path) {
        for entry in bytes.split(|&b| b == 0) {
            if let Ok(s) = std::str::from_utf8(entry) {
                if let Some((k, v)) = s.split_once('=') {
                    map.insert(k.to_string(), v.to_string());
                }
            }
        }
    }
    map
}

/// Reads the real UID of a process from `/proc/<pid>/status`.
/// Note: On SteamOS, `gamescope` and `kwin_wayland` carry `cap_sys_nice` file capabilities,
/// which sets `dumpable=0` and makes `/proc/<pid>` directory metadata appear owned by `root` (0)
/// even when running as `deck` (1000). Reading `Uid:` from `/proc/<pid>/status` handles this cleanly!
fn proc_real_uid(proc_path: &Path) -> Option<u32> {
    let status = fs::read_to_string(proc_path.join("status")).ok()?;
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("Uid:") {
            let ruid_str = rest.split_whitespace().next()?;
            return ruid_str.parse::<u32>().ok();
        }
    }
    None
}

fn discover_user_xauthority(my_uid: u32) -> Option<String> {
    let mut best: Option<(std::time::SystemTime, String)> = None;
    let run_dir = format!("/run/user/{my_uid}");
    for dir in [run_dir.as_str(), "/tmp"] {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let s = name.to_string_lossy();
                if !s.starts_with("xauth_") {
                    continue;
                }
                if let Ok(m) = entry.metadata() {
                    if m.uid() != my_uid {
                        continue;
                    }
                    let mtime = m.modified().unwrap_or(std::time::UNIX_EPOCH);
                    let path_str = entry.path().to_string_lossy().into_owned();
                    if best.as_ref().map_or(true, |(t, _)| mtime > *t) {
                        best = Some((mtime, path_str));
                    }
                }
            }
        }
    }
    if let Some((_, p)) = best {
        return Some(p);
    }
    if let Some(home) = std::env::var_os("HOME") {
        let fallback = Path::new(&home).join(".Xauthority");
        if fallback.exists() {
            return Some(fallback.to_string_lossy().into_owned());
        }
    }
    None
}

pub fn detect_active_session() -> SessionMode {
    let my_uid = unsafe { libc::getuid() };

    let mut gamescope_pid: Option<u32> = None;
    let mut kwin_x11_pid: Option<u32> = None;
    let mut kwin_wayland_pid: Option<u32> = None;

    if let Ok(entries) = fs::read_dir("/proc") {
        for entry in entries.flatten() {
            let fname = entry.file_name();
            let Some(pid_str) = fname.to_str() else { continue };
            let Ok(pid) = pid_str.parse::<u32>() else { continue };

            let path = entry.path();
            let comm = match fs::read_to_string(path.join("comm")) {
                Ok(c) => c,
                Err(_) => continue,
            };
            let comm = comm.trim();
            let is_gs = comm.starts_with("gamescope");
            if !is_gs && !matches!(comm, "kwin_x11" | "kwin_wayland") {
                continue;
            }

            if proc_real_uid(&path) != Some(my_uid) {
                continue;
            }

            if is_gs {
                gamescope_pid = Some(pid);
            } else if comm == "kwin_x11" {
                kwin_x11_pid = Some(pid);
            } else if comm == "kwin_wayland" {
                kwin_wayland_pid = Some(pid);
            }
        }
    }

    let is_desktop_running = kwin_x11_pid.is_some() || kwin_wayland_pid.is_some();

    if let Some(gs_pid) = gamescope_pid {
        if !is_desktop_running {
            debug!("Detected Gaming Mode (gamescope pid={gs_pid})");
            return SessionMode::GamingMode;
        }
    }

    if let Some(kx_pid) = kwin_x11_pid {
        let env = read_proc_environ(kx_pid);
        let display = env.get("DISPLAY").cloned().unwrap_or_else(|| ":0".to_string());
        let xauthority = env
            .get("XAUTHORITY")
            .cloned()
            .or_else(|| discover_user_xauthority(my_uid));
        if Path::new(&format!("/tmp/.X11-unix/X{}", display.trim_start_matches(':'))).exists() {
            return SessionMode::DesktopX11 {
                display,
                xauthority,
            };
        }
    }

    if let Some(kw_pid) = kwin_wayland_pid {
        let env = read_proc_environ(kw_pid);
        let dbus_addr = env
            .get("DBUS_SESSION_BUS_ADDRESS")
            .cloned()
            .or_else(|| Some(format!("unix:path=/run/user/{my_uid}/bus")));
        return SessionMode::DesktopWayland { dbus_addr };
    }

    SessionMode::Transitioning
}

/// Checks whether a remote Chrome Remote Desktop client is currently connected to `chrome-remote-desktop-host`
/// (or returns `true` if the host daemon isn't running yet so standalone diagnostics still stream).
pub fn is_crd_client_connected() -> bool {
    if std::env::var("STEAMOS_CRD_ALWAYS_ACTIVE").ok().as_deref() == Some("1") {
        return true;
    }

    let my_uid = unsafe { libc::getuid() };
    if Path::new(&format!("/run/user/{my_uid}/steamos-crd/force_active")).exists() {
        return true;
    }

    let Ok(entries) = fs::read_dir("/proc") else {
        return true;
    };

    let udp_inodes = collect_udp_socket_inodes();
    let mut found_host_proc = false;

    for entry in entries.flatten() {
        let fname = entry.file_name();
        let Some(pid_str) = fname.to_str() else { continue };
        if pid_str.parse::<u32>().is_err() {
            continue;
        }
        let path = entry.path();
        let exe_or_comm = fs::read_to_string(path.join("comm")).unwrap_or_default();
        let comm = exe_or_comm.trim();
        if comm != "chrome-remote-d" && comm != "remoting_me2me_" {
            continue;
        }
        if proc_real_uid(&path) != Some(my_uid) {
            continue;
        }
        found_host_proc = true;

        let fd_dir = path.join("fd");
        if let Ok(fds) = fs::read_dir(fd_dir) {
            for fd in fds.flatten() {
                if let Ok(target) = fs::read_link(fd.path()) {
                    let t_str = target.to_string_lossy();
                    if let Some(rest) = t_str.strip_prefix("socket:[") {
                        if let Some(inode_str) = rest.strip_suffix(']') {
                            if let Ok(inode) = inode_str.parse::<u64>() {
                                if udp_inodes.contains(&inode) {
                                    return true;
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    !found_host_proc
}

fn collect_udp_socket_inodes() -> HashSet<u64> {
    let mut set = HashSet::new();
    for path in ["/proc/net/udp", "/proc/net/udp6"] {
        if let Ok(content) = fs::read_to_string(path) {
            for line in content.lines().skip(1) {
                if let Some(inode_tok) = line.split_whitespace().nth(9) {
                    if let Ok(inode) = inode_tok.parse::<u64>() {
                        if inode != 0 {
                            set.insert(inode);
                        }
                    }
                }
            }
        }
    }
    set
}

/// Toggles `GAMESCOPE_COMPOSITE_FORCE` on `:0` so Steam Overlay and Quick Access Menu (`...`)
/// are merged into Gamescope's PipeWire stream while a remote client is connected, and restores
/// hardware plane bypass (`0`) when disconnected.
pub fn set_gamescope_composite_force(enable: bool) {
    let Ok((conn, screen_num)) = x11rb::connect(Some(":0")) else {
        return;
    };
    let root = conn.setup().roots[screen_num].root;
    let Ok(atom_cookie) = conn.intern_atom(false, b"GAMESCOPE_COMPOSITE_FORCE") else {
        return;
    };
    let Ok(atom_reply) = atom_cookie.reply() else {
        return;
    };
    let val: u32 = if enable { 1 } else { 0 };
    let _ = conn.change_property(
        PropMode::REPLACE,
        root,
        atom_reply.atom,
        AtomEnum::CARDINAL,
        32,
        1,
        &val.to_ne_bytes(),
    );
    let _ = conn.sync();
}
