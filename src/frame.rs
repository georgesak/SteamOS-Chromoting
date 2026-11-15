use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// libspa `enum spa_video_format` constants from `<spa/param/video/raw.h>`.
pub const SPA_VIDEO_FORMAT_BGRX: u32 = 8;
pub const SPA_VIDEO_FORMAT_RGBX: u32 = 7;
pub const SPA_VIDEO_FORMAT_RGBA: u32 = 11;
#[allow(dead_code)]
pub const SPA_VIDEO_FORMAT_BGRA: u32 = 12;

#[derive(Clone)]
pub struct FrameBuffer {
    pub width: u32,
    pub height: u32,
    /// Tightly packed `width * height * 4` bytes in little-endian `BGRx` (`[B, G, R, 0]`).
    pub bgrx: Vec<u8>,
}

pub struct LatestFrameSlot {
    seq: AtomicU64,
    inner: Mutex<Option<FrameBuffer>>,
}

impl LatestFrameSlot {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            seq: AtomicU64::new(0),
            inner: Mutex::new(None),
        })
    }

    pub fn publish_raw(
        &self,
        width: u32,
        height: u32,
        stride: u32,
        spa_format: u32,
        src: &[u8],
    ) {
        if width == 0 || height == 0 {
            return;
        }
        let row_bytes = (width as usize) * 4;
        let src_stride = (stride as usize).max(row_bytes);
        let needed = src_stride
            .saturating_mul((height as usize).saturating_sub(1))
            .saturating_add(row_bytes);
        if src.len() < needed {
            return;
        }

        let mut guard = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };

        let total_out = row_bytes * (height as usize);
        let buf = guard.get_or_insert_with(|| FrameBuffer {
            width,
            height,
            bgrx: vec![0u8; total_out],
        });

        if buf.width != width || buf.height != height || buf.bgrx.len() != total_out {
            buf.width = width;
            buf.height = height;
            buf.bgrx.resize(total_out, 0);
        }

        let swap_rb = matches!(spa_format, SPA_VIDEO_FORMAT_RGBX | SPA_VIDEO_FORMAT_RGBA);

        if !swap_rb && src_stride == row_bytes {
            buf.bgrx.copy_from_slice(&src[..total_out]);
        } else if !swap_rb {
            for y in 0..(height as usize) {
                let s_off = y * src_stride;
                let d_off = y * row_bytes;
                buf.bgrx[d_off..d_off + row_bytes].copy_from_slice(&src[s_off..s_off + row_bytes]);
            }
        } else {
            for y in 0..(height as usize) {
                let s_row = &src[y * src_stride..y * src_stride + row_bytes];
                let d_row = &mut buf.bgrx[y * row_bytes..(y + 1) * row_bytes];
                for (sp, dp) in s_row.chunks_exact(4).zip(d_row.chunks_exact_mut(4)) {
                    dp[0] = sp[2];
                    dp[1] = sp[1];
                    dp[2] = sp[0];
                    dp[3] = 0xFF;
                }
            }
        }

        self.seq.fetch_add(1, Ordering::Release);
    }

    pub fn sequence(&self) -> u64 {
        self.seq.load(Ordering::Acquire)
    }

    pub fn with_latest<F, R>(&self, f: F) -> Option<R>
    where
        F: FnOnce(&FrameBuffer) -> R,
    {
        let guard = match self.inner.lock() {
            Ok(g) => g,
            Err(poisoned) => poisoned.into_inner(),
        };
        guard.as_ref().map(f)
    }
}
