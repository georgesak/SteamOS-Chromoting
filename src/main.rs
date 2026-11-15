mod capture_pipewire;
mod capture_portal;
mod capture_x11;
mod clipboard;
mod frame;
mod input_uinput;
mod session_detector;
mod xvfb_presenter;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
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

fn start_capture_for_mode(
    mode: &SessionMode,
    frame_slot: &Arc<LatestFrameSlot>,
    active_client_flag: &Arc<AtomicBool>,
) -> ActiveCapture {
    match mode {
        SessionMode::GamingMode => {
            set_gamescope_composite_force(true);
            ActiveCapture::GamingPipeWire(PipeWireCaptureHandle::spawn_target_object(
                "gamescope",
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
            ))
        }
        SessionMode::DesktopWayland { dbus_addr } => {
            set_gamescope_composite_force(false);
            match PortalScreenCastSession::negotiate(dbus_addr.as_deref()) {
                Ok(portal) => {
                    // Split fields before moving pipewire_fd into capture thread while keeping `_portal` alive
                    let node_id = portal.node_id;
                    let dup_fd = unsafe { libc::fcntl(portal.pipewire_fd.as_raw_fd(), libc::F_DUPFD_CLOEXEC, 0) };
                    if dup_fd < 0 {
                        error!("Failed to dup portal PipeWire fd");
                        return ActiveCapture::None;
                    }
                    let owned_dup = unsafe { std::os::fd::OwnedFd::from_raw_fd(dup_fd) };
                    let pw = PipeWireCaptureHandle::spawn_portal_node(
                        owned_dup,
                        node_id,
                        Arc::clone(frame_slot),
                    );
                    ActiveCapture::DesktopWayland {
                        _portal: portal,
                        pw,
                    }
                }
                Err(e) => {
                    warn!("Desktop Wayland portal ScreenCast unavailable yet: {e:#}");
                    ActiveCapture::None
                }
            }
        }
        SessionMode::Transitioning => {
            set_gamescope_composite_force(false);
            ActiveCapture::None
        }
    }
}

use std::os::fd::{AsRawFd, FromRawFd};

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    install_signal_handlers();

    let xvfb_display =
        std::env::var("STEAMOS_CRD_DISPLAY").unwrap_or_else(|_| ":20".to_string());

    info!("Starting steamos-crd-bridge targeting Xvfb {xvfb_display}");

    let mut presenter = XvfbPresenter::connect(&xvfb_display)?;
    let mut uinput = UinputInjector::new()?;
    let mut clipboard = ClipboardBridge::new(&xvfb_display);

    presenter.present_splash("STEAMOS CHROMOTING", "READY - WAITING FOR SESSION")?;

    let frame_slot = LatestFrameSlot::new();
    let active_client_flag = Arc::new(AtomicBool::new(false));

    let mut current_mode: Option<SessionMode> = None;
    let mut capture = ActiveCapture::None;
    let mut last_presented_seq: u64 = 0;

    let mut last_probe = Instant::now() - Duration::from_secs(10);
    let mut last_clip_poll = Instant::now();
    let mut last_input_at = Instant::now() - Duration::from_secs(300);
    let mut last_capture_retry = Instant::now() - Duration::from_secs(10);

    while !SHUTDOWN.load(Ordering::Relaxed) {
        let now = Instant::now();
        let in_gaming_mode = matches!(current_mode, Some(SessionMode::GamingMode));

        // 1. Drain X11 input events on Xvfb -> /dev/uinput
        if presenter.pump_events(&mut uinput, in_gaming_mode)? {
            last_input_at = now;
        }

        // 2. Service clipboard requests rapidly
        if let Some(ref mut clip) = clipboard {
            clip.pump_requests();
            if now.duration_since(last_clip_poll) >= Duration::from_millis(750) {
                last_clip_poll = now;
                clip.poll_changes();
            }
        }

        // 3. Probe session mode & remote client state every 500ms
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

            let active_mode = current_mode.as_ref().unwrap();

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
                    capture = start_capture_for_mode(active_mode, &frame_slot, &active_client_flag);
                }
            }
        }

        // 4. Blit latest captured frame onto Xvfb if updated
        let seq = frame_slot.sequence();
        if seq != last_presented_seq {
            last_presented_seq = seq;
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
