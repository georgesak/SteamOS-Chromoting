use std::time::Duration;

use x11rb::connection::Connection;
use x11rb::protocol::Event;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ConnectionExt as _, CreateWindowAux, PropMode, SelectionNotifyEvent, Window,
    WindowClass, SELECTION_NOTIFY_EVENT,
};
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;

struct X11ClipEndpoint {
    conn: RustConnection,
    win: Window,
    clipboard: Atom,
    utf8: Atom,
    targets: Atom,
    prop: Atom,
}

impl X11ClipEndpoint {
    fn connect(display: &str) -> Option<Self> {
        let (conn, screen_num) = RustConnection::connect(Some(display)).ok()?;
        let root = conn.setup().roots[screen_num].root;
        let win = conn.generate_id().ok()?;
        conn.create_window(
            0,
            win,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new(),
        )
        .ok()?;

        let clipboard = conn.intern_atom(false, b"CLIPBOARD").ok()?.reply().ok()?.atom;
        let utf8 = conn.intern_atom(false, b"UTF8_STRING").ok()?.reply().ok()?.atom;
        let targets = conn.intern_atom(false, b"TARGETS").ok()?.reply().ok()?.atom;
        let prop = conn
            .intern_atom(false, b"STEAMOS_CRD_CLIP_PROP")
            .ok()?
            .reply()
            .ok()?
            .atom;
        let _ = conn.flush();

        Some(Self {
            conn,
            win,
            clipboard,
            utf8,
            targets,
            prop,
        })
    }

    /// Services incoming `SelectionRequest` events when this endpoint owns `CLIPBOARD`.
    /// Returns `Err(())` if the X11 connection was closed (e.g. during a SteamOS mode switch).
    fn pump_selection_requests(&self, text: &str) -> Result<(), ()> {
        loop {
            match self.conn.poll_for_event() {
                Ok(Some(Event::SelectionRequest(req))) => {
                    let dst_prop = if req.property == x11rb::NONE {
                        req.target
                    } else {
                        req.property
                    };
                    let mut out_prop = x11rb::NONE;

                    if req.target == self.targets {
                        let supported: [u32; 3] = [
                            self.targets,
                            self.utf8,
                            AtomEnum::STRING.into(),
                        ];
                        let mut bytes = Vec::with_capacity(12);
                        for a in supported {
                            bytes.extend_from_slice(&a.to_ne_bytes());
                        }
                        if self
                            .conn
                            .change_property(
                                PropMode::REPLACE,
                                req.requestor,
                                dst_prop,
                                AtomEnum::ATOM,
                                32,
                                3,
                                &bytes,
                            )
                            .is_ok()
                        {
                            out_prop = dst_prop;
                        }
                    } else if req.target == self.utf8 || req.target == AtomEnum::STRING.into() {
                        if self
                            .conn
                            .change_property8(
                                PropMode::REPLACE,
                                req.requestor,
                                dst_prop,
                                req.target,
                                text.as_bytes(),
                            )
                            .is_ok()
                        {
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
                    let _ = self.conn.send_event(false, req.requestor, 0u32.into(), notify);
                    let _ = self.conn.flush();
                }
                Ok(Some(_)) => {}
                Ok(None) => return Ok(()),
                Err(_) => return Err(()),
            }
        }
    }

    fn claim_clipboard(&self) {
        let _ = self
            .conn
            .set_selection_owner(self.win, self.clipboard, x11rb::CURRENT_TIME);
        let _ = self.conn.flush();
    }

    fn read_foreign_clipboard(&self) -> Option<String> {
        let owner = self.conn.get_selection_owner(self.clipboard).ok()?.reply().ok()?.owner;
        if owner == x11rb::NONE || owner == self.win {
            return None;
        }
        let _ = self.conn.delete_property(self.win, self.prop);
        self.conn
            .convert_selection(
                self.win,
                self.clipboard,
                self.utf8,
                self.prop,
                x11rb::CURRENT_TIME,
            )
            .ok()?;
        self.conn.flush().ok()?;

        let deadline = std::time::Instant::now() + Duration::from_millis(60);
        while std::time::Instant::now() < deadline {
            match self.conn.poll_for_event() {
                Ok(Some(Event::SelectionNotify(n))) => {
                    if n.property == x11rb::NONE {
                        return None;
                    }
                    let reply = self
                        .conn
                        .get_property(true, self.win, self.prop, AtomEnum::ANY, 0, 262_144)
                        .ok()?
                        .reply()
                        .ok()?;
                    return String::from_utf8(reply.value).ok();
                }
                Ok(Some(_)) => {}
                Ok(None) => std::thread::sleep(Duration::from_millis(4)),
                Err(_) => return None,
            }
        }
        None
    }
}

/// Bidirectional UTF-8 clipboard bridge between persistent `Xvfb :20` and active session `:0`.
pub struct ClipboardBridge {
    xvfb: X11ClipEndpoint,
    session: Option<X11ClipEndpoint>,
    shared_text: String,
}

impl ClipboardBridge {
    pub fn new(xvfb_display: &str) -> Option<Self> {
        let xvfb = X11ClipEndpoint::connect(xvfb_display)?;
        let session = X11ClipEndpoint::connect(":0");
        Some(Self {
            xvfb,
            session,
            shared_text: String::new(),
        })
    }

    /// Rapidly services pending `SelectionRequest` events on both X11 displays (non-blocking).
    pub fn pump_requests(&mut self) {
        let _ = self.xvfb.pump_selection_requests(&self.shared_text);
        if let Some(ref s) = self.session {
            if s.pump_selection_requests(&self.shared_text).is_err() {
                self.session = None;
            }
        }
    }

    /// Periodically checks for new clipboard ownership on either side and mirrors it to the other.
    pub fn poll_changes(&mut self) {
        if self.session.is_none() {
            self.session = X11ClipEndpoint::connect(":0");
        }
        self.pump_requests();

        if let Some(txt) = self.xvfb.read_foreign_clipboard() {
            if !txt.is_empty() && txt != self.shared_text {
                self.shared_text = txt;
                if let Some(ref s) = self.session {
                    s.claim_clipboard();
                }
                return;
            }
        }

        if let Some(ref s) = self.session {
            if let Some(txt) = s.read_foreign_clipboard() {
                if !txt.is_empty() && txt != self.shared_text {
                    self.shared_text = txt;
                    self.xvfb.claim_clipboard();
                }
            }
        }
    }
}
