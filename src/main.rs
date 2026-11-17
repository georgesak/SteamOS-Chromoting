mod capture_pipewire;
mod capture_portal;
mod capture_x11;
mod clipboard;
mod frame;
mod input_uinput;
mod session_detector;
mod xvfb_presenter;

use std::os::fd::{AsRawFd, FromRawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::Result;
use tracing::{error, info, warn};

use crate::capture_pipewire::PipeWireCaptureHandle;
use crate::capture_portal::PortalScreenCastSession;
use crate::capture_x11::X11CaptureHandle;
use crate::clipboard::ClipboardBridge;
use crate::frame::LatestFrameSlot;
use crate::input_uinput::UinputInjector;
use crate::session_detector::{
    detect_active_session, is_crd_client_connected, set_gamescope_composite_force, SessionMode,
};
use crate::xvfb_presenter::XvfbPresenter;

enum ActiveCapture {
    None,
    GamingPipeWire(PipeWireCaptureHandle),
    DesktopX11(X11CaptureHandle),
    DesktopWaylandPending(JoinHandle<Result<PortalScreenCastSession>>),
    DesktopWayland {
        _portal: PortalScreenCastSession,
        pw: PipeWireCaptureHandle,
    },
}

impl ActiveCapture {
    fn is_running(&self) -> bool {
        match self {
            ActiveCapture::None => false,
            ActiveCapture::GamingPipeWire(h) => !h.is_finished(),
            ActiveCapture::DesktopX11(h) => !h.is_finished(),
            ActiveCapture::DesktopWaylandPending(_) => true,
            ActiveCapture::DesktopWayland { pw, .. } => !pw.is_finished(),
        }
    }
}

static SHUTDOWN: AtomicBool = AtomicBool::new(false);

extern "C" fn on_term_signal(_sig: libc::c_int) {
    SHUTDOWN.store(true, Ordering::SeqCst);
}

fn install_signal_handlers() {
    unsafe {
        libc::signal(libc::SIGINT, on_term_signal as *const () as usize);
        libc::signal(libc::SIGTERM, on_term_signal as *const () as usize);
    }
}

#[derive(Debug, Clone, Copy)]
struct BridgeSettings {
    max_fps: u32,
    clipboard_sync: bool,
    gaming_chords: bool,
    composite_force: bool,
}

fn load_settings_env_defaults() {
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let path = std::path::Path::new(&home).join(".config/steamos-chromoting/settings.env");
    let Ok(contents) = std::fs::read_to_string(path) else {
        return;
    };
    for raw_line in contents.lines() {
        let line = raw_line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let line = line.strip_prefix("export ").map(str::trim).unwrap_or(line);
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        let key = k.trim();
        if key.is_empty() || std::env::var_os(key).is_some() {
            continue;
        }
        let val = v
            .trim()
            .trim_matches('"')
            .trim_matches('\'');
        std::env::set_var(key, val);
    }
}

fn env_bool(name: &str, default: bool) -> bool {
    match std::env::var(name) {
        Ok(v) => !matches!(
            v.trim().to_ascii_lowercase().as_str(),
            "0" | "false" | "no" | "off"
        ),
        Err(_) => default,
    }
}

impl BridgeSettings {
    fn from_env() -> Self {
        let max_fps = std::env::var("STEAMOS_CRD_MAX_FPS")
            .ok()
            .and_then(|s| s.trim().parse::<u32>().ok())
            .unwrap_or(60)
            .clamp(5, 120);
        Self {
            max_fps,
            clipboard_sync: env_bool("STEAMOS_CRD_CLIPBOARD_SYNC", true),
            gaming_chords: env_bool("STEAMOS_CRD_GAMING_CHORDS", true),
            composite_force: env_bool("STEAMOS_CRD_COMPOSITE_FORCE", true),
        }
    }
}

fn start_capture_for_mode(
    mode: &SessionMode,
    settings: BridgeSettings,
    frame_slot: &Arc<LatestFrameSlot>,
    active_client_flag: &Arc<AtomicBool>,
    presenter: &mut XvfbPresenter,
) -> ActiveCapture {
    match mode {
        SessionMode::GamingMode => {
            set_gamescope_composite_force(settings.composite_force);
            ActiveCapture::GamingPipeWire(PipeWireCaptureHandle::spawn_target_object(
                "gamescope",
                settings.max_fps,
                Arc::clone(frame_slot),
            ))
        }
        SessionMode::DesktopX11 {
            display,
            xauthority,
        } => {
            set_gamescope_composite_force(false);
            ActiveCapture::DesktopX11(X11CaptureHandle::spawn(
                display.clone(),
                xauthority.clone(),
                Arc::clone(frame_slot),
                Arc::clone(active_client_flag),
                settings.max_fps,
            ))
        }
        SessionMode::DesktopWayland { dbus_addr } => {
            set_gamescope_composite_force(false);
            let _ = presenter.present_splash(
                "KDE WAYLAND PORTAL APPROVAL NEEDED",
                "CLICK SHARE ON SCREEN OR USE DESKTOP X11",
            );
            let dbus_owned = dbus_addr.clone();
            let handle = thread::Builder::new()
                .name("portal-negotiate".to_string())
                .spawn(move || PortalScreenCastSession::negotiate(dbus_owned.as_deref()))
                .expect("spawn portal-negotiate thread");
            ActiveCapture::DesktopWaylandPending(handle)
        }
        SessionMode::Transitioning => {
            set_gamescope_composite_force(false);
            ActiveCapture::None
        }
    }
}

fn main() -> Result<()> {
    load_settings_env_defaults();

    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    install_signal_handlers();

    let xvfb_display =
        std::env::var("STEAMOS_CRD_DISPLAY").unwrap_or_else(|_| ":20".to_string());
    let settings = BridgeSettings::from_env();

    info!(
        "Starting steamos-crd-bridge targeting Xvfb {xvfb_display} (max_fps={}, clipboard={}, gaming_chords={}, composite_force={})",
        settings.max_fps,
        settings.clipboard_sync,
        settings.gaming_chords,
        settings.composite_force
    );

    let mut presenter = XvfbPresenter::connect(&xvfb_display)?;
    let mut uinput = UinputInjector::new()?;
    let mut clipboard = if settings.clipboard_sync {
        ClipboardBridge::new(&xvfb_display)
    } else {
        None
    };

    presenter.present_splash("STEAMOS CHROMOTING", "READY - WAITING FOR SESSION")?;

    let frame_slot = LatestFrameSlot::new();
    let active_client_flag = Arc::new(AtomicBool::new(false));

    let mut current_mode: Option<SessionMode> = None;
    let mut capture = ActiveCapture::None;
    let mut last_presented_seq: u64 = 0;
    let min_frame_interval = Duration::from_micros(1_000_000 / (settings.max_fps as u64));
    let mut last_blit_at = Instant::now() - Duration::from_secs(1);

    let mut last_probe = Instant::now() - Duration::from_secs(10);
    let mut last_input_at = Instant::now() - Duration::from_secs(300);
    let mut last_capture_retry = Instant::now() - Duration::from_secs(10);

    while !SHUTDOWN.load(Ordering::Relaxed) {
        let now = Instant::now();
        let in_gaming_mode = matches!(current_mode, Some(SessionMode::GamingMode));

        // 1. Service XFixes selection notifications & SelectionRequest events rapidly
        if let Some(ref mut clip) = clipboard {
            clip.pump();
        }

        // 2. Drain X11 input events on Xvfb -> /dev/uinput (with pre-paste clipboard sync)
        if presenter.pump_events(
            &mut uinput,
            clipboard.as_mut(),
            in_gaming_mode && settings.gaming_chords,
        )? {
            last_input_at = now;
        }

        // 3. Probe session mode, clipboard endpoints & remote client state every 500ms
        if now.duration_since(last_probe) >= Duration::from_millis(500) {
            last_probe = now;

            let recent_input = now.duration_since(last_input_at) < Duration::from_secs(30);
            let client_connected = recent_input || is_crd_client_connected();
            let was_connected = active_client_flag.swap(client_connected, Ordering::Relaxed);

            let detected_mode = detect_active_session();

            if current_mode.as_ref() != Some(&detected_mode) {
                let prev_label = current_mode
                    .as_ref()
                    .map(|m| m.label())
                    .unwrap_or("Startup");
                info!(
                    "SteamOS session active: '{}' -> '{}'",
                    prev_label,
                    detected_mode.label()
                );
                capture = ActiveCapture::None;
                let _ = presenter.present_splash("STEAMOS CHROMOTING", detected_mode.label());
                current_mode = Some(detected_mode);
            }

            if settings.clipboard_sync && clipboard.is_none() {
                clipboard = ClipboardBridge::new(&xvfb_display);
            }
            if let Some(ref mut clip) = clipboard {
                clip.maintenance(current_mode.as_ref());
            }

            let active_mode = current_mode.as_ref().unwrap();

            // Check if an async Wayland portal negotiation finished
            if let ActiveCapture::DesktopWaylandPending(ref h) = capture {
                if h.is_finished() {
                    let prev = std::mem::replace(&mut capture, ActiveCapture::None);
                    if let ActiveCapture::DesktopWaylandPending(join_h) = prev {
                        match join_h.join() {
                            Ok(Ok(portal)) => {
                                let node_id = portal.node_id;
                                let dup_fd = unsafe {
                                    libc::fcntl(portal.pipewire_fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0)
                                };
                                if dup_fd >= 0 {
                                    let owned_dup = unsafe { std::os::fd::OwnedFd::from_raw_fd(dup_fd) };
                                    let pw = PipeWireCaptureHandle::spawn_portal_node(
                                        owned_dup,
                                        node_id,
                                        settings.max_fps,
                                        Arc::clone(&frame_slot),
                                    );
                                    capture = ActiveCapture::DesktopWayland {
                                        _portal: portal,
                                        pw,
                                    };
                                } else {
                                    error!("Failed to dup portal PipeWire fd");
                                }
                            }
                            Ok(Err(e)) => {
                                warn!("Desktop Wayland portal ScreenCast unavailable yet: {e:#}");
                                last_capture_retry = now;
                            }
                            Err(_) => {
                                error!("Desktop Wayland portal negotiation thread panicked");
                                last_capture_retry = now;
                            }
                        }
                    }
                }
            }

            if !client_connected {
                if was_connected || capture.is_running() {
                    info!("No active CRD client connected; pausing capture for 0% idle overhead");
                    capture = ActiveCapture::None;
                    set_gamescope_composite_force(false);
                    let _ = presenter.present_splash("STEAMOS CHROMOTING", "IDLE - PAUSED UNTIL CLIENT CONNECTS");
                }
            } else {
                if !was_connected {
                    info!(
                        "CRD client active; starting capture for '{}'",
                        active_mode.label()
                    );
                }
                if !capture.is_running()
                    && !matches!(active_mode, SessionMode::Transitioning)
                    && now.duration_since(last_capture_retry) >= Duration::from_secs(1)
                {
                    last_capture_retry = now;
                    capture = start_capture_for_mode(
                        active_mode,
                        settings,
                        &frame_slot,
                        &active_client_flag,
                        &mut presenter,
                    );
                }
            }
        }

        // 4. Blit latest captured frame onto Xvfb if updated and frame interval elapsed
        let seq = frame_slot.sequence();
        if seq != last_presented_seq && now.duration_since(last_blit_at) >= min_frame_interval {
            last_presented_seq = seq;
            last_blit_at = now;
            let mut blit_res = Ok(());
            frame_slot.with_latest(|fb| {
                blit_res = presenter.present_frame(fb);
            });
            blit_res?;
        } else {
            // Sleep briefly: 4ms when client is active (~250Hz input polling), 50ms when idle
            if active_client_flag.load(Ordering::Relaxed) {
                thread::sleep(Duration::from_millis(4));
            } else {
                thread::sleep(Duration::from_millis(50));
            }
        }
    }

    info!("Shutting down steamos-crd-bridge cleanly...");
    drop(capture);
    set_gamescope_composite_force(false);
    Ok(())
}
