use std::ffi::{c_char, c_int, c_void, CString};
use std::os::fd::RawFd;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};

use tracing::{info, warn};

use crate::frame::LatestFrameSlot;

type PwFrameCb = unsafe extern "C" fn(
    userdata: *mut c_void,
    width: u32,
    height: u32,
    stride: u32,
    spa_format: u32,
    data: *const u8,
    size: usize,
);

extern "C" {
    fn steamos_pw_capture_run(
        portal_fd: c_int,
        target_node_id: u32,
        target_object: *const c_char,
        max_fps: u32,
        stop_flag: *const u8,
        on_frame: PwFrameCb,
        userdata: *mut c_void,
    ) -> c_int;
}

unsafe extern "C" fn frame_trampoline(
    userdata: *mut c_void,
    width: u32,
    height: u32,
    stride: u32,
    spa_format: u32,
    data: *const u8,
    size: usize,
) {
    if userdata.is_null() || data.is_null() || size == 0 {
        return;
    }
    let slot = &*(userdata as *const LatestFrameSlot);
    let slice = std::slice::from_raw_parts(data, size);
    slot.publish_raw(width, height, stride, spa_format, slice);
}

pub struct PipeWireCaptureHandle {
    stop_flag: Arc<AtomicU8>,
    join: Option<JoinHandle<()>>,
}

impl PipeWireCaptureHandle {
    pub fn spawn_target_object(
        target_object: &str,
        max_fps: u32,
        slot: Arc<LatestFrameSlot>,
    ) -> Self {
        let stop_flag = Arc::new(AtomicU8::new(0));
        let stop_clone = Arc::clone(&stop_flag);
        let target = CString::new(target_object).expect("valid CString target_object");
        let label = target_object.to_string();

        let join = thread::Builder::new()
            .name(format!("pw-cap-{label}"))
            .spawn(move || {
                info!("Starting PipeWire capture for target.object='{label}' (max_fps={max_fps})");
                let stop_ptr = stop_clone.as_ptr() as *const u8;
                let userdata = Arc::as_ptr(&slot) as *mut c_void;
                let rc = unsafe {
                    steamos_pw_capture_run(
                        -1,
                        0,
                        target.as_ptr(),
                        max_fps,
                        stop_ptr,
                        frame_trampoline,
                        userdata,
                    )
                };
                drop(slot);
                if rc != 0 && stop_clone.load(Ordering::Relaxed) == 0 {
                    warn!("PipeWire capture '{label}' exited with code {rc}");
                } else {
                    info!("PipeWire capture '{label}' stopped cleanly");
                }
            })
            .expect("spawn PipeWire capture thread");

        Self {
            stop_flag,
            join: Some(join),
        }
    }

    pub fn spawn_portal_node(
        portal_fd: std::os::fd::OwnedFd,
        node_id: u32,
        max_fps: u32,
        slot: Arc<LatestFrameSlot>,
    ) -> Self {
        use std::os::fd::AsRawFd;

        let stop_flag = Arc::new(AtomicU8::new(0));
        let stop_clone = Arc::clone(&stop_flag);

        let join = thread::Builder::new()
            .name(format!("pw-portal-{node_id}"))
            .spawn(move || {
                let raw_fd: RawFd = portal_fd.as_raw_fd();
                info!("Starting PipeWire portal capture on node_id={node_id} (fd={raw_fd}, max_fps={max_fps})");
                let stop_ptr = stop_clone.as_ptr() as *const u8;
                let userdata = Arc::as_ptr(&slot) as *mut c_void;
                let rc = unsafe {
                    steamos_pw_capture_run(
                        raw_fd,
                        node_id,
                        std::ptr::null(),
                        max_fps,
                        stop_ptr,
                        frame_trampoline,
                        userdata,
                    )
                };
                drop(portal_fd);
                drop(slot);
                if rc != 0 && stop_clone.load(Ordering::Relaxed) == 0 {
                    warn!("PipeWire portal capture (node {node_id}) exited with code {rc}");
                } else {
                    info!("PipeWire portal capture (node {node_id}) stopped cleanly");
                }
            })
            .expect("spawn PipeWire portal capture thread");

        Self {
            stop_flag,
            join: Some(join),
        }
    }

    pub fn is_finished(&self) -> bool {
        self.join.as_ref().map_or(true, |j| j.is_finished())
    }

    pub fn stop(&mut self) {
        self.stop_flag.store(1, Ordering::SeqCst);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for PipeWireCaptureHandle {
    fn drop(&mut self) {
        self.stop();
    }
}
