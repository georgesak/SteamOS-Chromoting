use anyhow::{Context, Result};
use tracing::info;
use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::shm::{self, ConnectionExt as _};
use x11rb::protocol::xproto::{
    ConfigureWindowAux, ConnectionExt as _, CreateGCAux, CreateWindowAux, EventMask, Gcontext,
    ImageFormat, InputFocus, StackMode, Window, WindowClass,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::frame::FrameBuffer;
use crate::input_uinput::UinputInjector;

struct ShmBuffer {
    addr: *mut u8,
    len: usize,
    seg: shm::Seg,
}

impl ShmBuffer {
    fn allocate(conn: &RustConnection, width: u32, height: u32) -> Result<Self> {
        let len = (width as usize) * (height as usize) * 4;
        let shmid = unsafe { libc::shmget(libc::IPC_PRIVATE, len, libc::IPC_CREAT | 0o600) };
        if shmid < 0 {
            anyhow::bail!("Presenter shmget({len}) failed: {}", std::io::Error::last_os_error());
        }
        let addr = unsafe { libc::shmat(shmid, std::ptr::null(), 0) };
        if addr == (-1isize as *mut libc::c_void) {
            unsafe { libc::shmctl(shmid, libc::IPC_RMID, std::ptr::null_mut()) };
            anyhow::bail!("Presenter shmat failed: {}", std::io::Error::last_os_error());
        }
        let seg = conn.generate_id()?;
        conn.shm_attach(seg, shmid as u32, false)?.check()?;
        unsafe { libc::shmctl(shmid, libc::IPC_RMID, std::ptr::null_mut()) };

        Ok(Self {
            addr: addr as *mut u8,
            len,
            seg,
        })
    }

    fn as_slice_mut(&mut self) -> &mut [u8] {
        unsafe { std::slice::from_raw_parts_mut(self.addr, self.len) }
    }
}

impl Drop for ShmBuffer {
    fn drop(&mut self) {
        if !self.addr.is_null() {
            unsafe { libc::shmdt(self.addr as *const libc::c_void) };
        }
    }
}

pub struct XvfbPresenter {
    pub conn: RustConnection,
    pub root: Window,
    pub window: Window,
    gc: Gcontext,
    depth: u8,
    pub width: u32,
    pub height: u32,
    shm: ShmBuffer,
    vp_x: i32,
    vp_y: i32,
    vp_w: u32,
    vp_h: u32,
    last_splash: Option<(String, String)>,
}

impl XvfbPresenter {
    pub fn connect(disp: &str) -> Result<Self> {
        let mut last_err = None;
        let mut conn_pair = None;
        for _ in 0..30 {
            match RustConnection::connect(Some(disp)) {
                Ok(pair) => {
                    conn_pair = Some(pair);
                    break;
                }
                Err(e) => {
                    last_err = Some(e);
                    std::thread::sleep(std::time::Duration::from_millis(150));
                }
            }
        }
        let (conn, screen_num) = conn_pair
            .ok_or_else(|| anyhow::anyhow!("Connect error: {:?}", last_err))
            .with_context(|| format!("Failed to connect to Xvfb display {disp}"))?;

        shm::query_version(&conn)?.reply().context("Xvfb missing MIT-SHM")?;

        let screen = &conn.setup().roots[screen_num];
        let root = screen.root;
        let depth = screen.root_depth;
        let width = screen.width_in_pixels as u32;
        let height = screen.height_in_pixels as u32;

        // Subscribe to root StructureNotify so we detect XRandR resizes triggered by CRD client
        conn.change_window_attributes(
            root,
            &x11rb::protocol::xproto::ChangeWindowAttributesAux::new()
                .event_mask(EventMask::STRUCTURE_NOTIFY),
        )?;

        let window = conn.generate_id()?;
        let win_aux = CreateWindowAux::new()
            .background_pixel(0x0010141A)
            .override_redirect(1)
            .event_mask(
                EventMask::KEY_PRESS
                    | EventMask::KEY_RELEASE
                    | EventMask::BUTTON_PRESS
                    | EventMask::BUTTON_RELEASE
                    | EventMask::POINTER_MOTION
                    | EventMask::EXPOSURE
                    | EventMask::FOCUS_CHANGE,
            );

        conn.create_window(
            depth,
            window,
            root,
            0,
            0,
            width as u16,
            height as u16,
            0,
            WindowClass::INPUT_OUTPUT,
            screen.root_visual,
            &win_aux,
        )?
        .check()?;

        conn.change_property8(
            x11rb::protocol::xproto::PropMode::REPLACE,
            window,
            x11rb::protocol::xproto::AtomEnum::WM_NAME,
            x11rb::protocol::xproto::AtomEnum::STRING,
            b"SteamOS-Chromoting Presenter",
        )?;

        conn.map_window(window)?;
        conn.set_input_focus(InputFocus::POINTER_ROOT, window, x11rb::CURRENT_TIME)?;

        let gc = conn.generate_id()?;
        conn.create_gc(gc, window, &CreateGCAux::new())?;

        let shm = ShmBuffer::allocate(&conn, width, height)?;
        conn.flush()?;

        info!("Connected Xvfb presenter on {disp} ({width}x{height}, depth={depth})");

        Ok(Self {
            conn,
            root,
            window,
            gc,
            depth,
            width,
            height,
            shm,
            vp_x: 0,
            vp_y: 0,
            vp_w: width,
            vp_h: height,
            last_splash: None,
        })
    }

    fn resize_surface(&mut self, new_w: u32, new_h: u32) -> Result<()> {
        if new_w == 0 || new_h == 0 || (new_w == self.width && new_h == self.height) {
            return Ok(());
        }
        info!("Xvfb root resized to {new_w}x{new_h}; resizing presenter surface");
        let _ = self.conn.shm_detach(self.shm.seg);
        self.width = new_w;
        self.height = new_h;
        self.conn.configure_window(
            self.window,
            &ConfigureWindowAux::new()
                .x(0)
                .y(0)
                .width(new_w)
                .height(new_h)
                .stack_mode(StackMode::ABOVE),
        )?;
        self.shm = ShmBuffer::allocate(&self.conn, new_w, new_h)?;
        self.vp_x = 0;
        self.vp_y = 0;
        self.vp_w = new_w;
        self.vp_h = new_h;
        if let Some((title, sub)) = self.last_splash.clone() {
            let _ = self.present_splash(&title, &sub);
        }
        self.conn.flush()?;
        Ok(())
    }

    /// Drains pending X11 events from Xvfb and forwards pointer/keyboard events to `/dev/uinput`.
    /// Returns `true` if any input activity occurred.
    pub fn pump_events(&mut self, uinput: &mut UinputInjector, in_gaming_mode: bool) -> Result<bool> {
        let mut had_input = false;

        while let Some(ev) = self.conn.poll_for_event()? {
            match ev {
                Event::MotionNotify(m) => {
                    had_input = true;
                    let (nx, ny) = self.normalize_coords(m.event_x as i32, m.event_y as i32);
                    uinput.move_pointer_normalized(nx, ny);
                }
                Event::ButtonPress(b) => {
                    had_input = true;
                    let (nx, ny) = self.normalize_coords(b.event_x as i32, b.event_y as i32);
                    uinput.move_pointer_normalized(nx, ny);
                    uinput.handle_x11_button(b.detail, true);
                }
                Event::ButtonRelease(b) => {
                    had_input = true;
                    uinput.handle_x11_button(b.detail, false);
                }
                Event::KeyPress(k) => {
                    had_input = true;
                    uinput.handle_x11_key(k.detail, true, in_gaming_mode);
                }
                Event::KeyRelease(k) => {
                    had_input = true;
                    uinput.handle_x11_key(k.detail, false, in_gaming_mode);
                }
                Event::FocusOut(_) => {
                    let _ = self.conn.set_input_focus(
                        InputFocus::POINTER_ROOT,
                        self.window,
                        x11rb::CURRENT_TIME,
                    );
                    let _ = self.conn.flush();
                }
                Event::ConfigureNotify(c) if c.window == self.root => {
                    had_input = true;
                    self.resize_surface(c.width as u32, c.height as u32)?;
                }
                _ => {}
            }
        }

        Ok(had_input)
    }

    fn normalize_coords(&self, x: i32, y: i32) -> (f64, f64) {
        let w = self.vp_w.max(1) as f64;
        let h = self.vp_h.max(1) as f64;
        let nx = ((x - self.vp_x) as f64 / w).clamp(0.0, 1.0);
        let ny = ((y - self.vp_y) as f64 / h).clamp(0.0, 1.0);
        (nx, ny)
    }

    pub fn present_frame(&mut self, frame: &FrameBuffer) -> Result<()> {
        let dst_w = self.width;
        let dst_h = self.height;
        let src_w = frame.width;
        let src_h = frame.height;
        if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
            return Ok(());
        }

        let dst = self.shm.as_slice_mut();

        if src_w == dst_w && src_h == dst_h {
            // Fast path: 1:1 zero-scale memcpy
            self.vp_x = 0;
            self.vp_y = 0;
            self.vp_w = dst_w;
            self.vp_h = dst_h;
            dst.copy_from_slice(&frame.bgrx);
        } else {
            // Aspect-ratio-preserving letterbox scale
            let scale_x = dst_w as f64 / src_w as f64;
            let scale_y = dst_h as f64 / src_h as f64;
            let scale = scale_x.min(scale_y);

            let vp_w = ((src_w as f64 * scale).round() as u32).clamp(1, dst_w);
            let vp_h = ((src_h as f64 * scale).round() as u32).clamp(1, dst_h);
            let vp_x = ((dst_w - vp_w) / 2) as i32;
            let vp_y = ((dst_h - vp_h) / 2) as i32;

            if vp_x != self.vp_x || vp_y != self.vp_y || vp_w != self.vp_w || vp_h != self.vp_h {
                dst.fill(0);
                self.vp_x = vp_x;
                self.vp_y = vp_y;
                self.vp_w = vp_w;
                self.vp_h = vp_h;
            }

            let dst_stride = (dst_w as usize) * 4;
            let src_stride = (src_w as usize) * 4;

            for vy in 0..vp_h {
                let sy = (((vy as u64) * (src_h as u64)) / (vp_h as u64)) as usize;
                let dy = (vp_y as u32 + vy) as usize;
                let s_row = &frame.bgrx[sy * src_stride..(sy + 1) * src_stride];
                let d_row = &mut dst[dy * dst_stride..(dy + 1) * dst_stride];
                let dx_start = vp_x as usize;

                for vx in 0..(vp_w as usize) {
                    let sx = ((vx * (src_w as usize)) / (vp_w as usize)) * 4;
                    let dx = (dx_start + vx) * 4;
                    d_row[dx..dx + 4].copy_from_slice(&s_row[sx..sx + 4]);
                }
            }
        }

        self.last_splash = None;
        self.blit_shm()?;
        Ok(())
    }

    pub fn present_splash(&mut self, title: &str, subtitle: &str) -> Result<()> {
        self.last_splash = Some((title.to_string(), subtitle.to_string()));
        let w = self.width as usize;
        let h = self.height as usize;
        let dst = self.shm.as_slice_mut();

        // Dark SteamOS deck slate gradient background
        for y in 0..h {
            let shade = (18 + (y * 18) / h.max(1)) as u8;
            let row = &mut dst[y * w * 4..(y + 1) * w * 4];
            for px in row.chunks_exact_mut(4) {
                px[0] = shade.saturating_add(14); // B
                px[1] = shade.saturating_add(4);  // G
                px[2] = shade;                    // R
                px[3] = 0xFF;
            }
        }

        // Accent bar
        let bar_y = h / 2 - 48;
        let bar_w = (w / 3).max(240).min(w);
        let bar_x = (w - bar_w) / 2;
        fill_rect(dst, w, h, bar_x, bar_y, bar_w, 4, [0xF5, 0x9E, 0x1A, 0xFF]);

        draw_centered_text(dst, w, h, (h / 2).saturating_sub(24), title, 3, [0xFF, 0xFF, 0xFF, 0xFF]);
        draw_centered_text(dst, w, h, h / 2 + 18, subtitle, 2, [0xC8, 0xB8, 0xA8, 0xFF]);

        self.blit_shm()?;
        Ok(())
    }

    fn blit_shm(&mut self) -> Result<()> {
        self.conn.shm_put_image(
            self.window,
            self.gc,
            self.width as u16,
            self.height as u16,
            0,
            0,
            self.width as u16,
            self.height as u16,
            0,
            0,
            self.depth,
            ImageFormat::Z_PIXMAP.into(),
            false,
            self.shm.seg,
            0,
        )?;
        self.conn.flush()?;
        Ok(())
    }
}

fn fill_rect(fb: &mut [u8], fb_w: usize, fb_h: usize, rx: usize, ry: usize, rw: usize, rh: usize, bgrx: [u8; 4]) {
    for y in ry..(ry + rh).min(fb_h) {
        for x in rx..(rx + rw).min(fb_w) {
            let off = (y * fb_w + x) * 4;
            fb[off..off + 4].copy_from_slice(&bgrx);
        }
    }
}

fn draw_centered_text(fb: &mut [u8], fb_w: usize, fb_h: usize, y: usize, text: &str, scale: usize, bgrx: [u8; 4]) {
    let upper = text.to_ascii_uppercase();
    let char_adv = 6 * scale;
    let total_w = upper.len().saturating_mul(char_adv);
    let start_x = fb_w.saturating_sub(total_w) / 2;

    for (i, ch) in upper.chars().enumerate() {
        let glyph = glyph_5x7(ch);
        let cx = start_x + i * char_adv;
        for (gy, row_bits) in glyph.iter().enumerate() {
            for gx in 0..5 {
                if (row_bits >> (4 - gx)) & 1 == 1 {
                    fill_rect(
                        fb,
                        fb_w,
                        fb_h,
                        cx + gx * scale,
                        y + gy * scale,
                        scale,
                        scale,
                        bgrx,
                    );
                }
            }
        }
    }
}

fn glyph_5x7(c: char) -> [u8; 7] {
    match c {
        'A' => [0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'B' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110],
        'C' => [0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110],
        'D' => [0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100],
        'E' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111],
        'F' => [0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000],
        'G' => [0b01110, 0b10001, 0b10000, 0b10111, 0b10001, 0b10001, 0b01110],
        'H' => [0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001],
        'I' => [0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
        'J' => [0b00111, 0b00010, 0b00010, 0b00010, 0b10010, 0b10010, 0b01100],
        'K' => [0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001],
        'L' => [0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111],
        'M' => [0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001],
        'N' => [0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001],
        'O' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'P' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000],
        'Q' => [0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101],
        'R' => [0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001],
        'S' => [0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110],
        'T' => [0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100],
        'U' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110],
        'V' => [0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100],
        'W' => [0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001],
        'X' => [0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001],
        'Y' => [0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100],
        'Z' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111],
        '0'..='9' => match c {
            '0' => [0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110],
            '1' => [0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110],
            '2' => [0b01110, 0b10001, 0b00001, 0b00110, 0b01000, 0b10000, 0b11111],
            '3' => [0b11110, 0b00001, 0b00001, 0b01110, 0b00001, 0b00001, 0b11110],
            '4' => [0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010],
            '5' => [0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110],
            '6' => [0b01110, 0b10000, 0b11110, 0b10001, 0b10001, 0b10001, 0b01110],
            '7' => [0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b01000, 0b01000],
            '8' => [0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110],
            _   => [0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110],
        },
        '-' => [0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000],
        '.' => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b01100, 0b01100],
        ':' => [0b00000, 0b01100, 0b01100, 0b00000, 0b01100, 0b01100, 0b00000],
        '(' => [0b00010, 0b00100, 0b01000, 0b01000, 0b01000, 0b00100, 0b00010],
        ')' => [0b01000, 0b00100, 0b00010, 0b00010, 0b00010, 0b00100, 0b01000],
        '/' => [0b00001, 0b00010, 0b00100, 0b00100, 0b01000, 0b10000, 0b00000],
        _   => [0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000],
    }
}
