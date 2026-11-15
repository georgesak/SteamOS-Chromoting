use std::collections::BTreeMap;
use std::sync::mpsc::{self, Receiver, SyncSender, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tracing::{debug, info};
use x11rb::connection::Connection;
use x11rb::protocol::xfixes::{self, ConnectionExt as _, SelectionEventMask};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, EventMask, PropMode, Property,
    SelectionNotifyEvent, SelectionRequestEvent, Window, WindowClass, SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

use crate::session_detector::{
    connect_x11_display, discover_session_x11_displays, SessionMode,
};

const MAX_CLIPBOARD_BYTES: usize = 4 * 1024 * 1024;

enum KlipperCmd {
    SetMode {
        active: bool,
        dbus_addr: Option<String>,
    },
    SetText(String),
}

struct KlipperBridge {
    cmd_tx: SyncSender<KlipperCmd>,
    incoming: Arc<Mutex<Option<String>>>,
    mode_active: bool,
    mode_dbus: Option<String>,
}

impl KlipperBridge {
    fn spawn() -> Self {
        let (cmd_tx, cmd_rx) = mpsc::sync_channel::<KlipperCmd>(16);
        let incoming = Arc::new(Mutex::new(None));
        let incoming_clone = Arc::clone(&incoming);

        let _ = thread::Builder::new()
            .name("crd-klipper".to_string())
            .spawn(move || run_klipper_worker(cmd_rx, incoming_clone));

        Self {
            cmd_tx,
            incoming,
            mode_active: false,
            mode_dbus: None,
        }
    }

    fn update_mode(&mut self, mode: Option<&SessionMode>) {
        let (active, dbus_addr) = match mode {
            Some(SessionMode::DesktopWayland { dbus_addr }) => (true, dbus_addr.clone()),
            Some(SessionMode::DesktopX11 { .. }) => (true, None),
            _ => (false, None),
        };
        if active != self.mode_active || dbus_addr != self.mode_dbus {
            self.mode_active = active;
            self.mode_dbus = dbus_addr.clone();
            let _ = self.cmd_tx.try_send(KlipperCmd::SetMode { active, dbus_addr });
        }
    }

    fn set_text(&self, text: &str) {
        if self.mode_active {
            let _ = self.cmd_tx.try_send(KlipperCmd::SetText(text.to_string()));
        }
    }

    fn take_incoming(&self) -> Option<String> {
        self.incoming.lock().ok()?.take()
    }
}

fn connect_session_dbus(dbus_addr: Option<&str>) -> Option<zbus::blocking::Connection> {
    let my_uid = unsafe { libc::getuid() };
    let default_addr = format!("unix:path=/run/user/{my_uid}/bus");
    let addr = dbus_addr.unwrap_or(&default_addr);
    zbus::blocking::connection::Builder::address(addr)
        .ok()?
        .build()
        .ok()
}

fn klipper_has_owner(conn: &zbus::blocking::Connection) -> Option<bool> {
    let msg = conn
        .call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus"),
            "NameHasOwner",
            &("org.kde.klipper",),
        )
        .ok()?;
    msg.body().deserialize::<bool>().ok()
}

fn klipper_get_contents(conn: &zbus::blocking::Connection) -> Option<String> {
    let msg = conn
        .call_method(
            Some("org.kde.klipper"),
            "/klipper",
            Some("org.kde.klipper.klipper"),
            "getClipboardContents",
            &(),
        )
        .ok()?;
    msg.body().deserialize::<String>().ok()
}

fn klipper_set_contents(conn: &zbus::blocking::Connection, text: &str) -> bool {
    conn.call_method(
        Some("org.kde.klipper"),
        "/klipper",
        Some("org.kde.klipper.klipper"),
        "setClipboardContents",
        &(text,),
    )
    .is_ok()
}

fn run_klipper_worker(rx: Receiver<KlipperCmd>, incoming: Arc<Mutex<Option<String>>>) {
    let mut active = false;
    let mut dbus_addr: Option<String> = None;
    let mut conn: Option<zbus::blocking::Connection> = None;
    let mut last_seen = String::new();

    let apply_cmd = |cmd: KlipperCmd,
                     active: &mut bool,
                     dbus_addr: &mut Option<String>,
                     conn: &mut Option<zbus::blocking::Connection>,
                     last_seen: &mut String| {
        match cmd {
            KlipperCmd::SetMode {
                active: new_active,
                dbus_addr: new_addr,
            } => {
                if !new_active || *dbus_addr != new_addr {
                    *conn = None;
                }
                *active = new_active;
                *dbus_addr = new_addr;
            }
            KlipperCmd::SetText(text) => {
                *last_seen = text.clone();
                if !*active {
                    return;
                }
                if conn.is_none() {
                    *conn = connect_session_dbus(dbus_addr.as_deref());
                }
                if let Some(ref c) = *conn {
                    match klipper_has_owner(c) {
                        Some(true) => {
                            if !klipper_set_contents(c, &text) {
                                *conn = None;
                            }
                        }
                        Some(false) => {}
                        None => {
                            *conn = None;
                        }
                    }
                }
            }
        }
    };

    loop {
        let timeout = if active {
            Duration::from_millis(250)
        } else {
            Duration::from_secs(2)
        };

        match rx.recv_timeout(timeout) {
            Ok(first_cmd) => {
                apply_cmd(
                    first_cmd,
                    &mut active,
                    &mut dbus_addr,
                    &mut conn,
                    &mut last_seen,
                );
                while let Ok(next_cmd) = rx.try_recv() {
                    apply_cmd(
                        next_cmd,
                        &mut active,
                        &mut dbus_addr,
                        &mut conn,
                        &mut last_seen,
                    );
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }

        if !active {
            continue;
        }

        if conn.is_none() {
            conn = connect_session_dbus(dbus_addr.as_deref());
        }
        let Some(ref c) = conn else { continue };

        match klipper_has_owner(c) {
            Some(true) => {
                if let Some(txt) = klipper_get_contents(c) {
                    if !txt.is_empty() && txt != last_seen {
                        last_seen = txt.clone();
                        if let Ok(mut slot) = incoming.lock() {
                            *slot = Some(txt);
                        }
                    }
                }
            }
            Some(false) => {}
            None => {
                conn = None;
            }
        }
    }
}

fn intern(conn: &RustConnection, name: &[u8]) -> Option<Atom> {
    conn.intern_atom(false, name).ok()?.reply().ok().map(|r| r.atom)
}

struct X11ClipEndpoint {
    display: String,
    conn: RustConnection,
    win: Window,
    clipboard: Atom,
    primary: Atom,
    targets: Atom,
    timestamp: Atom,
    multiple: Atom,
    save_targets: Atom,
    incr: Atom,
    utf8: Atom,
    text_atom: Atom,
    text_plain_utf8: Atom,
    text_plain: Atom,
    prop: Atom,
    time_prop: Atom,
    owns_clipboard: bool,
    acquired_time: u32,
    foreign_changed: bool,
    has_xfixes: bool,
}

impl X11ClipEndpoint {
    fn connect_xvfb(display: &str) -> Option<Self> {
        let (conn, screen_num) = RustConnection::connect(Some(display)).ok()?;
        Self::init(display.to_string(), conn, screen_num)
    }

    fn connect_session(display: &str, explicit_xauth: Option<&str>) -> Option<Self> {
        let (conn, screen_num) = connect_x11_display(display, explicit_xauth)?;
        Self::init(display.to_string(), conn, screen_num)
    }

    fn init(disp_name: String, conn: RustConnection, screen_num: usize) -> Option<Self> {
        let screen = conn.setup().roots.get(screen_num)?;
        let root = screen.root;
        let depth = screen.root_depth;
        let visual = screen.root_visual;

        let win = conn.generate_id().ok()?;
        conn.create_window(
            depth,
            win,
            root,
            -10,
            -10,
            1,
            1,
            0,
            WindowClass::INPUT_OUTPUT,
            visual,
            &CreateWindowAux::new()
                .override_redirect(1)
                .event_mask(EventMask::PROPERTY_CHANGE),
        )
        .ok()?
        .check()
        .ok()?;

        let clipboard = intern(&conn, b"CLIPBOARD")?;
        let primary = AtomEnum::PRIMARY.into();
        let targets = intern(&conn, b"TARGETS")?;
        let timestamp = intern(&conn, b"TIMESTAMP")?;
        let multiple = intern(&conn, b"MULTIPLE")?;
        let save_targets = intern(&conn, b"SAVE_TARGETS")?;
        let incr = intern(&conn, b"INCR")?;
        let utf8 = intern(&conn, b"UTF8_STRING")?;
        let text_atom = intern(&conn, b"TEXT")?;
        let text_plain_utf8 = intern(&conn, b"text/plain;charset=utf-8")?;
        let text_plain = intern(&conn, b"text/plain")?;
        let prop = intern(&conn, b"STEAMOS_CRD_CLIP_PROP")?;
        let time_prop = intern(&conn, b"STEAMOS_CRD_TIME_PROP")?;

        let has_xfixes = if xfixes::query_version(&conn, 5, 0)
            .ok()
            .and_then(|c| c.reply().ok())
            .is_some()
        {
            conn.xfixes_select_selection_input(
                win,
                clipboard,
                SelectionEventMask::SET_SELECTION_OWNER,
            )
            .is_ok()
        } else {
            false
        };

        let _ = conn.flush();

        debug!("Connected clipboard endpoint on {disp_name} (xfixes={has_xfixes})");

        Some(Self {
            display: disp_name,
            conn,
            win,
            clipboard,
            primary,
            targets,
            timestamp,
            multiple,
            save_targets,
            incr,
            utf8,
            text_atom,
            text_plain_utf8,
            text_plain,
            prop,
            time_prop,
            owns_clipboard: false,
            acquired_time: x11rb::CURRENT_TIME,
            foreign_changed: false,
            has_xfixes,
        })
    }

    fn fulfill_single_target(
        &self,
        requestor: Window,
        target: Atom,
        dst_prop: Atom,
        text: &str,
    ) -> bool {
        if dst_prop == x11rb::NONE {
            return false;
        }

        if target == self.targets {
            let supported: [u32; 8] = [
                self.targets,
                self.timestamp,
                self.save_targets,
                self.utf8,
                self.text_plain_utf8,
                self.text_plain,
                self.text_atom,
                AtomEnum::STRING.into(),
            ];
            self.conn
                .change_property32(
                    PropMode::REPLACE,
                    requestor,
                    dst_prop,
                    AtomEnum::ATOM,
                    &supported,
                )
                .is_ok()
        } else if target == self.timestamp {
            let ts = [self.acquired_time];
            self.conn
                .change_property32(
                    PropMode::REPLACE,
                    requestor,
                    dst_prop,
                    AtomEnum::INTEGER,
                    &ts,
                )
                .is_ok()
        } else if target == self.save_targets {
            self.conn
                .change_property8(PropMode::REPLACE, requestor, dst_prop, AtomEnum::ATOM, &[])
                .is_ok()
        } else if target == self.utf8
            || target == self.text_plain_utf8
            || target == self.text_plain
            || target == self.text_atom
            || target == AtomEnum::STRING.into()
        {
            // Per ICCCM, polymorphic TEXT target resolves to the actual encoding atom (UTF8_STRING).
            let actual_type: Atom = if target == self.text_atom {
                self.utf8
            } else {
                target
            };
            let bytes = truncate_utf8_bytes(text, MAX_CLIPBOARD_BYTES);
            self.conn
                .change_property8(PropMode::REPLACE, requestor, dst_prop, actual_type, bytes)
                .is_ok()
        } else {
            false
        }
    }

    fn reply_selection_request(&self, req: SelectionRequestEvent, text: &str) {
        let dst_prop = if req.property == x11rb::NONE {
            req.target
        } else {
            req.property
        };

        let mut out_prop = x11rb::NONE;

        if req.selection == self.clipboard || req.selection == self.primary {
            if req.target == self.multiple && req.property != x11rb::NONE {
                if let Ok(cookie) = self.conn.get_property(
                    false,
                    req.requestor,
                    req.property,
                    AtomEnum::ANY,
                    0,
                    1024,
                ) {
                    if let Ok(reply) = cookie.reply() {
                        if reply.format == 32 && reply.value.len() % 8 == 0 {
                            let mut updated_pairs: Vec<u32> =
                                Vec::with_capacity(reply.value.len() / 4);
                            for chunk in reply.value.chunks_exact(8) {
                                let t = u32::from_ne_bytes([
                                    chunk[0], chunk[1], chunk[2], chunk[3],
                                ]);
                                let p = u32::from_ne_bytes([
                                    chunk[4], chunk[5], chunk[6], chunk[7],
                                ]);
                                let ok = self.fulfill_single_target(req.requestor, t, p, text);
                                updated_pairs.push(t);
                                updated_pairs.push(if ok { p } else { x11rb::NONE });
                            }
                            if self
                                .conn
                                .change_property32(
                                    PropMode::REPLACE,
                                    req.requestor,
                                    req.property,
                                    reply.type_,
                                    &updated_pairs,
                                )
                                .is_ok()
                            {
                                out_prop = req.property;
                            }
                        }
                    }
                }
            } else if self.fulfill_single_target(req.requestor, req.target, dst_prop, text) {
                out_prop = dst_prop;
            }
        }

        let notify = SelectionNotifyEvent {
            response_type: SELECTION_NOTIFY_EVENT,
            sequence: 0,
            time: req.time,
            requestor: req.requestor,
            selection: req.selection,
            target: req.target,
            property: out_prop,
        };
        let _ = self
            .conn
            .send_event(false, req.requestor, EventMask::NO_EVENT, notify);
        let _ = self.conn.flush();
    }

    fn handle_background_event(&mut self, ev: Event, shared_text: &str) {
        match ev {
            Event::SelectionRequest(req) if req.owner == self.win => {
                self.reply_selection_request(req, shared_text);
            }
            Event::SelectionClear(clr)
                if clr.owner == self.win && clr.selection == self.clipboard =>
            {
                self.owns_clipboard = false;
            }
            Event::XfixesSelectionNotify(xf)
                if xf.window == self.win && xf.selection == self.clipboard =>
            {
                if xf.owner != x11rb::NONE && xf.owner != self.win {
                    self.owns_clipboard = false;
                    self.foreign_changed = true;
                } else if xf.owner == x11rb::NONE {
                    self.owns_clipboard = false;
                }
            }
            _ => {}
        }
    }

    /// Services all pending X11 events without blocking. Returns `Err(())` if X11 disconnected.
    fn pump_events(&mut self, shared_text: &str) -> Result<(), ()> {
        loop {
            match self.conn.poll_for_event() {
                Ok(Some(ev)) => self.handle_background_event(ev, shared_text),
                Ok(None) => return Ok(()),
                Err(_) => return Err(()),
            }
        }
    }

    fn take_foreign_changed(&mut self) -> bool {
        let was = self.foreign_changed;
        self.foreign_changed = false;
        was
    }

    fn query_server_time(&mut self, shared_text: &str) -> u32 {
        if self
            .conn
            .change_property8(
                PropMode::APPEND,
                self.win,
                self.time_prop,
                AtomEnum::ATOM,
                &[],
            )
            .is_err()
        {
            return x11rb::CURRENT_TIME;
        }
        if self.conn.flush().is_err() {
            return x11rb::CURRENT_TIME;
        }

        let deadline = Instant::now() + Duration::from_millis(15);
        while Instant::now() < deadline {
            match self.conn.poll_for_event() {
                Ok(Some(Event::PropertyNotify(p)))
                    if p.window == self.win && p.atom == self.time_prop =>
                {
                    return if p.time == 0 {
                        x11rb::CURRENT_TIME
                    } else {
                        p.time
                    };
                }
                Ok(Some(ev)) => self.handle_background_event(ev, shared_text),
                Ok(None) => thread::sleep(Duration::from_millis(1)),
                Err(_) => return x11rb::CURRENT_TIME,
            }
        }
        x11rb::CURRENT_TIME
    }

    fn claim_clipboard(
        &mut self,
        shared_text: &str,
        also_primary: bool,
        sync_immediately: bool,
    ) -> Result<(), ()> {
        let ts = self.query_server_time(shared_text);
        self.acquired_time = ts;
        self.conn
            .set_selection_owner(self.win, self.clipboard, ts)
            .map_err(|_| ())?;
        if also_primary {
            let _ = self.conn.set_selection_owner(self.win, self.primary, ts);
        }
        if sync_immediately {
            self.conn.sync().map_err(|_| ())?;
        } else {
            self.conn.flush().map_err(|_| ())?;
        }
        self.owns_clipboard = true;
        self.foreign_changed = false;
        Ok(())
    }

    fn has_foreign_owner(&self) -> bool {
        let Some(owner) = self
            .conn
            .get_selection_owner(self.clipboard)
            .ok()
            .and_then(|c| c.reply().ok())
            .map(|r| r.owner)
        else {
            return false;
        };
        owner != x11rb::NONE && owner != self.win
    }

    fn read_foreign_clipboard(&mut self, shared_text: &str) -> Option<String> {
        if !self.has_foreign_owner() {
            return None;
        }

        let candidate_targets: [Atom; 4] = [
            self.utf8,
            self.text_plain_utf8,
            self.text_plain,
            AtomEnum::STRING.into(),
        ];

        for target in candidate_targets {
            if let Some(val) = self.request_target(target, shared_text) {
                if !val.is_empty() {
                    return Some(val);
                }
            }
        }

        None
    }

    fn request_target(&mut self, target: Atom, shared_text: &str) -> Option<String> {
        let _ = self.conn.delete_property(self.win, self.prop);
        self.conn
            .convert_selection(
                self.win,
                self.clipboard,
                target,
                self.prop,
                x11rb::CURRENT_TIME,
            )
            .ok()?;
        self.conn.flush().ok()?;

        let deadline = Instant::now() + Duration::from_millis(80);
        while Instant::now() < deadline {
            match self.conn.poll_for_event() {
                Ok(Some(Event::SelectionNotify(n)))
                    if n.requestor == self.win && n.selection == self.clipboard =>
                {
                    if n.property == x11rb::NONE {
                        return None;
                    }
                    let reply = self
                        .conn
                        .get_property(true, self.win, n.property, AtomEnum::ANY, 0, 1_048_576)
                        .ok()?
                        .reply()
                        .ok()?;

                    if reply.type_ == self.incr {
                        return self.read_incr_stream(n.property, shared_text);
                    }
                    if reply.format != 8 {
                        return None;
                    }
                    return Some(decode_text_bytes(reply.value));
                }
                Ok(Some(ev)) => self.handle_background_event(ev, shared_text),
                Ok(None) => thread::sleep(Duration::from_millis(2)),
                Err(_) => return None,
            }
        }
        None
    }

    fn read_incr_stream(&mut self, prop: Atom, shared_text: &str) -> Option<String> {
        let mut acc: Vec<u8> = Vec::new();
        let deadline = Instant::now() + Duration::from_millis(300);

        while Instant::now() < deadline {
            match self.conn.poll_for_event() {
                Ok(Some(Event::PropertyNotify(p)))
                    if p.window == self.win
                        && p.atom == prop
                        && p.state == Property::NEW_VALUE =>
                {
                    let reply = self
                        .conn
                        .get_property(true, self.win, prop, AtomEnum::ANY, 0, 1_048_576)
                        .ok()?
                        .reply()
                        .ok()?;
                    // Ignore stale PropertyNotify queued before the initial INCR delete
                    if reply.type_ == x11rb::NONE {
                        continue;
                    }
                    if reply.value.is_empty() {
                        return Some(decode_text_bytes(acc));
                    }
                    if acc.len() + reply.value.len() <= MAX_CLIPBOARD_BYTES {
                        acc.extend_from_slice(&reply.value);
                    }
                }
                Ok(Some(ev)) => self.handle_background_event(ev, shared_text),
                Ok(None) => thread::sleep(Duration::from_millis(2)),
                Err(_) => return None,
            }
        }
        None
    }
}

fn truncate_utf8_bytes(s: &str, max_bytes: usize) -> &[u8] {
    if s.len() <= max_bytes {
        return s.as_bytes();
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s.as_bytes()[..end]
}

fn decode_text_bytes(bytes: Vec<u8>) -> String {
    match String::from_utf8(bytes) {
        Ok(s) => s,
        // Legacy X11 STRING is ISO-8859-1 (Latin-1), which maps 1:1 to Unicode U+0000..U+00FF
        Err(e) => e.into_bytes().into_iter().map(|b| b as char).collect(),
    }
}

/// Bidirectional UTF-8 clipboard bridge between persistent `Xvfb :20` (Chrome Remote Desktop),
/// all active SteamOS X11/Xwayland displays (`:0` Steam GamepadUI, `:1` Gamescope game/app,
/// or Desktop Xorg/Xwayland), and KDE Plasma's `org.kde.klipper` D-Bus service (for Wayland & X11).
pub struct ClipboardBridge {
    xvfb_display: String,
    xvfb: X11ClipEndpoint,
    sessions: BTreeMap<String, X11ClipEndpoint>,
    klipper: KlipperBridge,
    shared_text: String,
}

impl ClipboardBridge {
    pub fn new(xvfb_display: &str) -> Option<Self> {
        let xvfb = X11ClipEndpoint::connect_xvfb(xvfb_display)?;
        let klipper = KlipperBridge::spawn();
        let mut bridge = Self {
            xvfb_display: xvfb_display.to_string(),
            xvfb,
            sessions: BTreeMap::new(),
            klipper,
            shared_text: String::new(),
        };
        bridge.refresh_session_endpoints(None);
        if bridge.xvfb.has_foreign_owner() {
            bridge.pull_from_xvfb(false);
        }
        info!("Initialized bidirectional clipboard bridge (Xvfb {xvfb_display} <-> SteamOS)");
        Some(bridge)
    }

    /// Rapidly services `SelectionRequest` and `XFixesSelectionNotify` events across Xvfb and
    /// all active SteamOS session displays (~250Hz when a remote client is connected).
    pub fn pump(&mut self) {
        let _ = self.xvfb.pump_events(&self.shared_text);

        let shared_snapshot = self.shared_text.clone();
        self.sessions
            .retain(|_, ep| ep.pump_events(&shared_snapshot).is_ok());

        // 1. Remote client -> SteamOS (triggered immediately via XFixes on Xvfb :20)
        if self.xvfb.take_foreign_changed() {
            self.pull_from_xvfb(false);
        }

        // 2. SteamOS X11/Xwayland (:0, :1, ...) -> Remote client & peer session displays
        let mut changed_source: Option<(String, String)> = None;
        let disp_keys: Vec<String> = self.sessions.keys().cloned().collect();
        for disp in disp_keys {
            let should_read = self
                .sessions
                .get_mut(&disp)
                .map_or(false, |ep| ep.take_foreign_changed());
            if should_read {
                let read_res = self
                    .sessions
                    .get_mut(&disp)
                    .and_then(|ep| ep.read_foreign_clipboard(&self.shared_text));
                if let Some(txt) = read_res {
                    if !txt.is_empty() && txt != self.shared_text {
                        changed_source = Some((disp, txt));
                        break;
                    }
                }
            }
        }

        if let Some((src_disp, txt)) = changed_source {
            self.shared_text = txt.clone();
            let _ = self.xvfb.claim_clipboard(&self.shared_text, false, false);
            self.klipper.set_text(&self.shared_text);

            // Mirror across peer Gamescope Xwayland servers (e.g. :0 <-> :1)
            let mut dead = Vec::new();
            for (disp, ep) in &mut self.sessions {
                if disp != &src_disp {
                    if ep.claim_clipboard(&self.shared_text, true, false).is_err() {
                        dead.push(disp.clone());
                    }
                }
            }
            for d in dead {
                self.sessions.remove(&d);
            }
        }

        // 3. KDE Plasma Wayland / X11 Klipper D-Bus -> Remote client (:20)
        if let Some(txt) = self.klipper.take_incoming() {
            if !txt.is_empty() && txt != self.shared_text {
                self.shared_text = txt;
                let _ = self.xvfb.claim_clipboard(&self.shared_text, false, false);
            }
        }
    }

    /// Ensures any pending clipboard update from the remote client on `:20` is committed to all
    /// SteamOS session displays right before a paste shortcut (`Ctrl+V` / `Shift+Insert`) is injected.
    pub fn sync_before_paste(&mut self) {
        let _ = self.xvfb.pump_events(&self.shared_text);
        let xvfb_changed = self.xvfb.take_foreign_changed();

        if xvfb_changed || (!self.xvfb.has_xfixes && self.xvfb.has_foreign_owner()) {
            self.pull_from_xvfb(true);
        }
    }

    /// Discovers newly started X11/Xwayland displays (e.g., `:1` when a game launches in Gaming Mode
    /// or `:0` after switching between Gaming and Desktop Mode), updates Klipper D-Bus mode, and
    /// provides a fallback poll if XFixes is unavailable on any display.
    pub fn maintenance(&mut self, mode: Option<&SessionMode>) {
        self.klipper.update_mode(mode);
        self.refresh_session_endpoints(mode);
        self.pump();

        if !self.xvfb.has_xfixes {
            if let Some(txt) = self.xvfb.read_foreign_clipboard(&self.shared_text) {
                if !txt.is_empty() && txt != self.shared_text {
                    self.shared_text = txt;
                    self.push_to_sessions(false);
                    return;
                }
            }
        }

        let mut polled_update: Option<(String, String)> = None;
        let disp_keys: Vec<String> = self.sessions.keys().cloned().collect();
        for disp in disp_keys {
            let needs_poll = self.sessions.get(&disp).map_or(false, |ep| !ep.has_xfixes);
            if needs_poll {
                if let Some(txt) = self
                    .sessions
                    .get_mut(&disp)
                    .and_then(|ep| ep.read_foreign_clipboard(&self.shared_text))
                {
                    if !txt.is_empty() && txt != self.shared_text {
                        polled_update = Some((disp, txt));
                        break;
                    }
                }
            }
        }

        if let Some((src_disp, txt)) = polled_update {
            self.shared_text = txt;
            let _ = self.xvfb.claim_clipboard(&self.shared_text, false, false);
            self.klipper.set_text(&self.shared_text);
            for (disp, ep) in &mut self.sessions {
                if disp != &src_disp {
                    let _ = ep.claim_clipboard(&self.shared_text, true, false);
                }
            }
        }
    }

    fn pull_from_xvfb(&mut self, sync_immediately: bool) {
        if let Some(txt) = self.xvfb.read_foreign_clipboard(&self.shared_text) {
            if !txt.is_empty() {
                self.shared_text = txt;
                self.push_to_sessions(sync_immediately);
            }
        }
    }

    fn push_to_sessions(&mut self, sync_immediately: bool) {
        let mut dead = Vec::new();
        for (disp, ep) in &mut self.sessions {
            if ep
                .claim_clipboard(&self.shared_text, true, sync_immediately)
                .is_err()
            {
                dead.push(disp.clone());
            }
        }
        for d in dead {
            self.sessions.remove(&d);
        }
        self.klipper.set_text(&self.shared_text);
    }

    fn refresh_session_endpoints(&mut self, mode: Option<&SessionMode>) {
        let explicit_xauth = match mode {
            Some(SessionMode::DesktopX11 { xauthority, .. }) => xauthority.as_deref(),
            _ => None,
        };

        let mut candidates = discover_session_x11_displays(&self.xvfb_display);
        if let Some(SessionMode::DesktopX11 { display, .. }) = mode {
            if !candidates.contains(display) && display != &self.xvfb_display {
                candidates.push(display.clone());
            }
        }

        for disp in candidates {
            if self.sessions.contains_key(&disp) {
                continue;
            }
            if let Some(mut ep) = X11ClipEndpoint::connect_session(&disp, explicit_xauth) {
                if !self.shared_text.is_empty() {
                    let _ = ep.claim_clipboard(&self.shared_text, true, false);
                } else if let Some(initial_txt) = ep.read_foreign_clipboard(&self.shared_text) {
                    if !initial_txt.is_empty() {
                        self.shared_text = initial_txt;
                        let _ = self.xvfb.claim_clipboard(&self.shared_text, false, false);
                    }
                }
                let _ = &ep.display;
                self.sessions.insert(disp, ep);
            }
        }
    }
}
