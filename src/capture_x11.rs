use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use tracing::{info, warn};
use x11rb::connection::Connection;
use x11rb::protocol::shm::{self, ConnectionExt as _};
use x11rb::protocol::xfixes::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{ConnectionExt as _, ImageFormat};
use x11rb::rust_connection::RustConnection;

use crate::frame::{LatestFrameSlot, SPA_VIDEO_FORMAT_BGRX};

pub struct X11CaptureHandle {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl X11CaptureHandle {
    pub fn spawn(
        disp: String,
        xauthority: Option<String>,
        slot: Arc<LatestFrameSlot>,
        active_client: Arc<AtomicBool>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_clone = Arc::clone(&stop);

        let join = thread::Builder::new()
            .name(format!("x11-cap-{disp}"))
            .spawn(move || {
                if let Err(e) = run_x11_capture(&disp, xauthority.as_deref(), &slot, &stop_clone, &active_client) {
                    if !stop_clone.load(Ordering::Relaxed) {
                        warn!("X11 desktop capture on {disp} ended: {e:#}");
                    }
                }
            })
            .expect("spawn x11 capture thread");

        Self {
            stop,
            join: Some(join),
        }
    }

    pub fn is_finished(&self) -> bool {
        self.join.as_ref().map_or(true, |j| j.is_finished())
    }

    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for X11CaptureHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

struct ShmSegment {
    shmid: i32,
    addr: *mut u8,
    len: usize,
    seg: shm::Seg,
}

impl ShmSegment {
    fn allocate(conn: &RustConnection, len: usize) -> Result<Self> {
        let shmid = unsafe { libc::shmget(libc::IPC_PRIVATE, len, libc::IPC_CREAT | 0o600) };
        if shmid < 0 {
            anyhow::bail!("shmget({len}) failed: {}", std::io::Error::last_os_error());
        }
        let addr = unsafe { libc::shmat(shmid, std::ptr::null(), 0) };
        if addr == (-1isize as *mut libc::c_void) {
            unsafe { libc::shmctl(shmid, libc::IPC_RMID, std::ptr::null_mut()) };
            anyhow::bail!("shmat failed: {}", std::io::Error::last_os_error());
        }
        let seg = conn.generate_id()?;
        conn.shm_attach(seg, shmid as u32, false)?.check()?;
        // Mark for removal immediately once attached so kernel frees it even on crash
        unsafe { libc::shmctl(shmid, libc::IPC_RMID, std::ptr::null_mut()) };

        Ok(Self {
            shmid,
            addr: addr as *mut u8,
            len,
            seg,
        })
    }
}

impl Drop for ShmSegment {
    fn drop(&mut self) {
        let _ = self.shmid;
        if !self.addr.is_null() {
            unsafe { libc::shmdt(self.addr as *const libc::c_void) };
        }
    }
}

fn run_x11_capture(
    disp: &str,
    xauthority: Option<&str>,
    slot: &LatestFrameSlot,
    stop: &AtomicBool,
    active_client: &AtomicBool,
) -> Result<()> {
    let (conn, screen_num) = crate::session_detector::connect_x11_display(disp, xauthority)
        .ok_or_else(|| anyhow::anyhow!("Failed to connect to Desktop X11 display {disp}"))?;

    shm::query_version(&conn)?.reply().context("MIT-SHM not supported on X11 display")?;
    let has_xfixes = xfixes::query_version(&conn, 5, 0)
        .ok()
        .and_then(|c| c.reply().ok())
        .is_some();

    let root = conn.setup().roots[screen_num].root;
    let mut cur_w = conn.setup().roots[screen_num].width_in_pixels as u32;
    let mut cur_h = conn.setup().roots[screen_num].height_in_pixels as u32;
    let mut shm = ShmSegment::allocate(&conn, (cur_w as usize) * (cur_h as usize) * 4)?;

    info!("Started X11 Desktop Mode capture on {disp} ({cur_w}x{cur_h}, cursor={has_xfixes})");

    let mut frame_scratch = vec![0u8; shm.len];

    while !stop.load(Ordering::Relaxed) {
        let target_interval = if active_client.load(Ordering::Relaxed) {
            Duration::from_millis(16) // ~60 FPS
        } else {
            Duration::from_millis(500) // 2 FPS idle thumbnail heartbeat
        };
        let tick_start = Instant::now();

        let geom = conn.get_geometry(root)?.reply()?;
        let w = geom.width as u32;
        let h = geom.height as u32;
        if w != cur_w || h != cur_h {
            let _ = conn.shm_detach(shm.seg);
            cur_w = w;
            cur_h = h;
            shm = ShmSegment::allocate(&conn, (cur_w as usize) * (cur_h as usize) * 4)?;
            frame_scratch.resize(shm.len, 0);
            info!("Desktop X11 resolution changed to {cur_w}x{cur_h}");
        }

        conn.shm_get_image(
            root,
            0,
            0,
            cur_w as u16,
            cur_h as u16,
            !0,
            ImageFormat::Z_PIXMAP.into(),
            shm.seg,
            0,
        )?
        .reply()?;

        let raw_slice = unsafe { std::slice::from_raw_parts(shm.addr, shm.len) };
        frame_scratch.copy_from_slice(raw_slice);

        if has_xfixes {
            if let Ok(cookie) = conn.xfixes_get_cursor_image() {
                if let Ok(cursor) = cookie.reply() {
                    overlay_cursor(
                        &mut frame_scratch,
                        cur_w as i32,
                        cur_h as i32,
                        cursor.x as i32 - cursor.xhot as i32,
                        cursor.y as i32 - cursor.yhot as i32,
                        cursor.width as i32,
                        cursor.height as i32,
                        &cursor.cursor_image,
                    );
                }
            }
        }

        slot.publish_raw(cur_w, cur_h, cur_w * 4, SPA_VIDEO_FORMAT_BGRX, &frame_scratch);

        let elapsed = tick_start.elapsed();
        if elapsed < target_interval {
            thread::sleep(target_interval - elapsed);
        }
    }

    let _ = conn.shm_detach(shm.seg);
    Ok(())
}

fn overlay_cursor(
    fb: &mut [u8],
    fb_w: i32,
    fb_h: i32,
    cx: i32,
    cy: i32,
    cw: i32,
    ch: i32,
    argb: &[u32],
) {
    for row in 0..ch {
        let dy = cy + row;
        if dy < 0 || dy >= fb_h {
            continue;
        }
        for col in 0..cw {
            let dx = cx + col;
            if dx < 0 || dx >= fb_w {
                continue;
            }
            let idx = (row * cw + col) as usize;
            if idx >= argb.len() {
                continue;
            }
            let px = argb[idx];
            let a = ((px >> 24) & 0xFF) as u32;
            if a == 0 {
                continue;
            }
            let r = ((px >> 16) & 0xFF) as u32;
            let g = ((px >> 8) & 0xFF) as u32;
            let b = (px & 0xFF) as u32;

            let dst = ((dy * fb_w + dx) as usize) * 4;
            if a == 255 {
                fb[dst] = b as u8;
                fb[dst + 1] = g as u8;
                fb[dst + 2] = r as u8;
                fb[dst + 3] = 0xFF;
            } else {
                let inv_a = 255 - a;
                // XFixes cursor pixels are premultiplied ARGB
                fb[dst] = (b + (fb[dst] as u32 * inv_a) / 255).min(255) as u8;
                fb[dst + 1] = (g + (fb[dst + 1] as u32 * inv_a) / 255).min(255) as u8;
                fb[dst + 2] = (r + (fb[dst + 2] as u32 * inv_a) / 255).min(255) as u8;
                fb[dst + 3] = 0xFF;
            }
        }
    }
}
