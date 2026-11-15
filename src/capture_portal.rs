use std::collections::HashMap;
use std::fs;
use std::os::fd::OwnedFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use anyhow::{anyhow, Context, Result};
use tracing::{info, warn};
use zbus::blocking::{Connection, MessageIterator, Proxy};
use zbus::MatchRule;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

static TOKEN_COUNTER: AtomicU64 = AtomicU64::new(1);

fn next_token(prefix: &str) -> String {
    let n = TOKEN_COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("steamos_crd_{prefix}_{}_{}", std::process::id(), n)
}

fn token_file_path() -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(PathBuf::from(home).join(".config/steamos-chromoting/portal_token"))
}

fn load_restore_token() -> Option<String> {
    let path = token_file_path()?;
    let tok = fs::read_to_string(path).ok()?;
    let trimmed = tok.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}

fn save_restore_token(token: &str) {
    if token.trim().is_empty() {
        return;
    }
    if let Some(path) = token_file_path() {
        if let Some(parent) = path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        let _ = fs::write(path, token.trim());
    }
}

fn expected_request_path(conn: &Connection, handle_token: &str) -> Result<OwnedObjectPath> {
    let unique = conn
        .unique_name()
        .ok_or_else(|| anyhow!("D-Bus connection missing unique_name"))?;
    let sanitized = unique.as_str().trim_start_matches(':').replace('.', "_");
    let path_str = format!("/org/freedesktop/portal/desktop/request/{sanitized}/{handle_token}");
    Ok(ObjectPath::try_from(path_str.as_str())?.into())
}

fn wait_portal_response(
    iter: &mut MessageIterator,
) -> Result<(u32, HashMap<String, OwnedValue>)> {
    for msg in iter.by_ref() {
        let msg = msg.context("Error reading portal Response signal")?;
        let header = msg.header();
        if header.member().map(|m| m.as_str()) == Some("Response") {
            let body: (u32, HashMap<String, OwnedValue>) = msg
                .body()
                .deserialize()
                .context("Failed to deserialize portal Response body")?;
            return Ok(body);
        }
    }
    Err(anyhow!("D-Bus signal stream closed before portal Response arrived"))
}

pub struct PortalScreenCastSession {
    _conn: Connection,
    session_path: OwnedObjectPath,
    pub pipewire_fd: OwnedFd,
    pub node_id: u32,
}

impl PortalScreenCastSession {
    pub fn negotiate(dbus_addr: Option<&str>) -> Result<Self> {
        if let Some(addr) = dbus_addr {
            std::env::set_var("DBUS_SESSION_BUS_ADDRESS", addr);
        }

        let conn = Connection::session()
            .context("Failed to connect to session D-Bus for ScreenCast portal")?;

        let proxy = Proxy::new(
            &conn,
            "org.freedesktop.portal.Desktop",
            "/org/freedesktop/portal/desktop",
            "org.freedesktop.portal.ScreenCast",
        )
        .context("Failed to create ScreenCast portal proxy")?;

        // 1. CreateSession
        let req_tok_1 = next_token("create");
        let sess_tok = next_token("sess");
        let req_path_1 = expected_request_path(&conn, &req_tok_1)?;
        let rule_1 = MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.freedesktop.portal.Request")?
            .member("Response")?
            .path(req_path_1.as_ref())?
            .build();
        let mut iter_1 = MessageIterator::for_match_rule(rule_1, &conn, Some(16))?;

        let mut create_opts: HashMap<&str, Value<'_>> = HashMap::new();
        create_opts.insert("handle_token", Value::from(req_tok_1.as_str()));
        create_opts.insert("session_handle_token", Value::from(sess_tok.as_str()));

        let _: OwnedObjectPath = proxy
            .call("CreateSession", &(create_opts,))
            .context("ScreenCast.CreateSession failed")?;

        let (rc1, res1) = wait_portal_response(&mut iter_1)?;
        if rc1 != 0 {
            anyhow::bail!("ScreenCast.CreateSession rejected with code {rc1}");
        }

        let session_handle_str: String = res1
            .get("session_handle")
            .ok_or_else(|| anyhow!("Missing session_handle in CreateSession response"))?
            .try_clone()?
            .try_into()
            .or_else(|_| -> Result<String> {
                let p: OwnedObjectPath = res1
                    .get("session_handle")
                    .unwrap()
                    .try_clone()?
                    .try_into()?;
                Ok(p.to_string())
            })?;

        let session_path: OwnedObjectPath = ObjectPath::try_from(session_handle_str.as_str())?.into();

        // 2. SelectSources (types=1 MONITOR, cursor_mode=2 EMBEDDED, persist_mode=2 persistent)
        let req_tok_2 = next_token("select");
        let req_path_2 = expected_request_path(&conn, &req_tok_2)?;
        let rule_2 = MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.freedesktop.portal.Request")?
            .member("Response")?
            .path(req_path_2.as_ref())?
            .build();
        let mut iter_2 = MessageIterator::for_match_rule(rule_2, &conn, Some(16))?;

        let cached_token = load_restore_token();
        let mut select_opts: HashMap<&str, Value<'_>> = HashMap::new();
        select_opts.insert("handle_token", Value::from(req_tok_2.as_str()));
        select_opts.insert("types", Value::U32(1)); // 1 = MONITOR
        select_opts.insert("multiple", Value::Bool(false));
        select_opts.insert("cursor_mode", Value::U32(2)); // 2 = EMBEDDED cursor
        select_opts.insert("persist_mode", Value::U32(2)); // 2 = persist until explicitly revoked
        if let Some(ref tok) = cached_token {
            info!("Using cached Desktop Wayland portal restore_token");
            select_opts.insert("restore_token", Value::from(tok.as_str()));
        }

        let _: OwnedObjectPath = proxy
            .call("SelectSources", &(session_path.as_ref(), select_opts))
            .context("ScreenCast.SelectSources failed")?;

        let (rc2, _) = wait_portal_response(&mut iter_2)?;
        if rc2 != 0 {
            anyhow::bail!("ScreenCast.SelectSources rejected with code {rc2}");
        }

        // 3. Start
        let req_tok_3 = next_token("start");
        let req_path_3 = expected_request_path(&conn, &req_tok_3)?;
        let rule_3 = MatchRule::builder()
            .msg_type(zbus::message::Type::Signal)
            .interface("org.freedesktop.portal.Request")?
            .member("Response")?
            .path(req_path_3.as_ref())?
            .build();
        let mut iter_3 = MessageIterator::for_match_rule(rule_3, &conn, Some(16))?;

        let mut start_opts: HashMap<&str, Value<'_>> = HashMap::new();
        start_opts.insert("handle_token", Value::from(req_tok_3.as_str()));

        info!("Requesting ScreenCast.Start (approve once on KDE Desktop if first-time Wayland setup)...");
        let _: OwnedObjectPath = proxy
            .call("Start", &(session_path.as_ref(), "", start_opts))
            .context("ScreenCast.Start failed")?;

        let (rc3, res3) = wait_portal_response(&mut iter_3)?;
        if rc3 != 0 {
            // Clear stale restore_token if user cancelled or token became invalid
            if cached_token.is_some() {
                warn!("Clearing rejected portal restore_token so next attempt can re-prompt cleanly");
                if let Some(p) = token_file_path() {
                    let _ = fs::remove_file(p);
                }
            }
            anyhow::bail!("ScreenCast.Start cancelled/rejected with code {rc3}");
        }

        if let Some(tok_val) = res3.get("restore_token") {
            if let Ok(tok_str) = <String>::try_from(tok_val.try_clone()?) {
                save_restore_token(&tok_str);
                info!("Saved persistent Desktop Wayland portal restore_token");
            }
        }

        let streams_val = res3
            .get("streams")
            .ok_or_else(|| anyhow!("No streams returned in ScreenCast.Start"))?
            .try_clone()?;

        let streams: Vec<(u32, HashMap<String, OwnedValue>)> = streams_val
            .try_into()
            .context("Failed to decode ScreenCast streams array")?;
        let (node_id, _) = streams
            .first()
            .ok_or_else(|| anyhow!("ScreenCast returned empty streams list"))?;
        let node_id = *node_id;

        // 4. OpenPipeWireRemote
        let empty_opts: HashMap<&str, Value<'_>> = HashMap::new();
        let pw_zfd: zbus::zvariant::OwnedFd = proxy
            .call("OpenPipeWireRemote", &(session_path.as_ref(), empty_opts))
            .context("ScreenCast.OpenPipeWireRemote failed")?;
        let pipewire_fd: OwnedFd = pw_zfd.into();

        info!("Negotiated Desktop Wayland ScreenCast stream on PipeWire node {node_id}");

        Ok(Self {
            _conn: conn,
            session_path,
            pipewire_fd,
            node_id,
        })
    }

    pub fn close_session(&self) {
        if let Ok(sess_proxy) = Proxy::new(
            &self._conn,
            "org.freedesktop.portal.Desktop",
            self.session_path.as_ref(),
            "org.freedesktop.portal.Session",
        ) {
            let _: std::result::Result<(), _> = sess_proxy.call("Close", &());
        }
    }
}

impl Drop for PortalScreenCastSession {
    fn drop(&mut self) {
        self.close_session();
    }
}
