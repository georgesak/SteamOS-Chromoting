use std::collections::{HashMap, HashSet};
use std::fs;
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::SystemTime;

use tracing::debug;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{AtomEnum, ConnectionExt as _, PropMode};
use x11rb::rust_connection::{DefaultStream, RustConnection};
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

fn discover_xauth_candidates(my_uid: u32, disp_num: &str, explicit: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut push_unique = |p: String| {
        if !p.is_empty() && !out.contains(&p) {
            out.push(p);
        }
    };

    if let Some(exp) = explicit {
        push_unique(exp.to_string());
    }

    // Check running Xorg / Xwayland command lines for `-auth <file>` matching this display
    if let Ok(entries) = fs::read_dir("/proc") {
        let disp_arg = format!(":{disp_num}");
        for entry in entries.flatten() {
            let fname = entry.file_name();
            let Some(pid_str) = fname.to_str() else { continue };
            if pid_str.parse::<u32>().is_err() {
                continue;
            }
            let path = entry.path();
            let Ok(comm) = fs::read_to_string(path.join("comm")) else { continue };
            let comm = comm.trim();
            if !matches!(comm, "Xwayland" | "Xorg" | "X") {
                continue;
            }
            let Ok(cmdline) = fs::read(path.join("cmdline")) else { continue };
            let args: Vec<&str> = cmdline
                .split(|&b| b == 0)
                .filter(|s| !s.is_empty())
                .filter_map(|s| std::str::from_utf8(s).ok())
                .collect();
            let matches_disp = args.iter().any(|&a| a == disp_arg);
            if matches_disp {
                for win in args.windows(2) {
                    if win[0] == "-auth" {
                        push_unique(win[1].to_string());
                    }
                }
            }
        }
    }

    // Collect user-owned xauth_* files in /run/user/<uid> and /tmp ordered newest first
    let mut timed: Vec<(SystemTime, String)> = Vec::new();
    let run_dir = format!("/run/user/{my_uid}");
    for dir in [run_dir.as_str(), "/tmp"] {
        if let Ok(entries) = fs::read_dir(dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let s = name.to_string_lossy();
                if !s.starts_with("xauth_") && !s.starts_with(".xauth") {
                    continue;
                }
                if let Ok(m) = entry.metadata() {
                    if m.uid() != my_uid {
                        continue;
                    }
                    let mtime = m.modified().unwrap_or(SystemTime::UNIX_EPOCH);
                    timed.push((mtime, entry.path().to_string_lossy().into_owned()));
                }
            }
        }
    }
    timed.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, p) in timed {
        push_unique(p);
    }

    if let Some(home) = std::env::var_os("HOME") {
        let fallback = Path::new(&home).join(".Xauthority");
        if fallback.exists() {
            push_unique(fallback.to_string_lossy().into_owned());
        }
    }

    out
}

fn discover_user_xauthority(my_uid: u32) -> Option<String> {
    discover_xauth_candidates(my_uid, "0", None).into_iter().next()
}

fn read_u16_be(buf: &[u8], pos: &mut usize) -> Option<u16> {
    if *pos + 2 > buf.len() {
        return None;
    }
    let val = u16::from_be_bytes([buf[*pos], buf[*pos + 1]]);
    *pos += 2;
    Some(val)
}

fn read_xauth_field<'a>(buf: &'a [u8], pos: &mut usize) -> Option<&'a [u8]> {
    let len = read_u16_be(buf, pos)? as usize;
    if *pos + len > buf.len() {
        return None;
    }
    let slice = &buf[*pos..*pos + len];
    *pos += len;
    Some(slice)
}

/// Parses an `.Xauthority` binary file (libXau format) and returns `(auth_name, auth_data)` pairs
/// matching `disp_num` (exact display match prioritized before wildcard entries).
fn parse_xauth_cookies(path: &Path, disp_num: &str) -> Vec<(Vec<u8>, Vec<u8>)> {
    let Ok(bytes) = fs::read(path) else {
        return Vec::new();
    };
    let mut pos = 0;
    let mut exact = Vec::new();
    let mut wildcard = Vec::new();

    while pos < bytes.len() {
        let Some(family) = read_u16_be(&bytes, &mut pos) else { break };
        let Some(_addr) = read_xauth_field(&bytes, &mut pos) else { break };
        let Some(num) = read_xauth_field(&bytes, &mut pos) else { break };
        let Some(name) = read_xauth_field(&bytes, &mut pos) else { break };
        let Some(data) = read_xauth_field(&bytes, &mut pos) else { break };

        // FamilyLocal (256), FamilyWild (65535, used by KWin Xwayland), or Internet/LocalHost (0)
        if (family == 256 || family == 65535 || family == 0) && !name.is_empty() && !data.is_empty() {
            if num == disp_num.as_bytes() {
                exact.push((name.to_vec(), data.to_vec()));
            } else if num.is_empty() {
                wildcard.push((name.to_vec(), data.to_vec()));
            }
        }
    }

    exact.extend(wildcard);
    exact
}

/// Connects to an X11 display without mutating process-global `$XAUTHORITY`, automatically
/// authenticating with SDDM / KWin Xwayland cookies when required and falling back to unauthenticated
/// connections for Gamescope and Xvfb.
pub fn connect_x11_display(
    display: &str,
    explicit_xauth: Option<&str>,
) -> Option<(RustConnection, usize)> {
    let trimmed = display.trim_start_matches(':');
    let (disp_num, screen_str) = trimmed.split_once('.').unwrap_or((trimmed, "0"));
    let screen_num: usize = screen_str.parse().unwrap_or(0);
    let sock_path = format!("/tmp/.X11-unix/X{disp_num}");

    let my_uid = unsafe { libc::getuid() };
    let candidate_files = discover_xauth_candidates(my_uid, disp_num, explicit_xauth);

    let mut cookies: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
    for file in &candidate_files {
        for pair in parse_xauth_cookies(Path::new(file), disp_num) {
            if !cookies.contains(&pair) {
                cookies.push(pair);
            }
        }
    }

    for (auth_name, auth_data) in cookies {
        if let Ok(unix) = UnixStream::connect(&sock_path) {
            if let Ok((stream, _)) = DefaultStream::from_unix_stream(unix) {
                if let Ok(conn) = RustConnection::connect_to_stream_with_auth_info(
                    stream,
                    screen_num,
                    auth_name,
                    auth_data,
                ) {
                    return Some((conn, screen_num));
                }
            }
        }
    }

    if let Ok(unix) = UnixStream::connect(&sock_path) {
        if let Ok((stream, _)) = DefaultStream::from_unix_stream(unix) {
            if let Ok(conn) = RustConnection::connect_to_stream(stream, screen_num) {
                return Some((conn, screen_num));
            }
        }
    }

    RustConnection::connect(Some(display)).ok()
}

/// Discovers all active X11 display sockets in `/tmp/.X11-unix` other than our persistent Xvfb
/// display (e.g., both `:0` Steam GamepadUI and `:1` active game/app Xwayland in Gaming Mode,
/// or `:0`/`:1` in Desktop Mode).
pub fn discover_session_x11_displays(xvfb_display: &str) -> Vec<String> {
    let xvfb_num = xvfb_display
        .trim_start_matches(':')
        .split('.')
        .next()
        .unwrap_or("20");

    let mut nums: Vec<u32> = Vec::new();
    if let Ok(entries) = fs::read_dir("/tmp/.X11-unix") {
        for entry in entries.flatten() {
            let fname = entry.file_name();
            let Some(s) = fname.to_str() else { continue };
            let Some(num_str) = s.strip_prefix('X') else { continue };
            if num_str == xvfb_num {
                continue;
            }
            if let Ok(n) = num_str.parse::<u32>() {
                nums.push(n);
            }
        }
    }

    nums.sort_unstable();
    nums.dedup();
    nums.into_iter().map(|n| format!(":{n}")).collect()
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
    let Some((conn, screen_num)) = connect_x11_display(":0", None) else {
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
    let _ = conn.change_property32(
        PropMode::REPLACE,
        root,
        atom_reply.atom,
        AtomEnum::CARDINAL,
        &[val],
    );
    let _ = conn.sync();
}
