//! [`DsiPanel`]: the DSI output as a kumiki-gfx
//! [`FramebufferPanel`](kumiki_gfx::FramebufferPanel) (feature `gfx`).
//!
//! Triple-buffered. The DMA scans the front buffer; the CPU (or the PPA)
//! draws into the back buffer; the third one was the front until the
//! previous present. `present` points the DMA at the back buffer, and the
//! buffer retired by the *previous* present becomes the new back buffer
//! once it has been out of the scan for two frames (see [`Dsi::show`]).
//! With one present per frame that wait is at most about one frame, where a
//! double buffer would wait two on every present.
//!
//! The new back buffer holds the frame from two presents ago; the areas
//! changed by the last two presents are copied over from the front, so it
//! again holds the frame on screen and drawing continues incrementally.
//!
//! Large fills and copies go to the PPA. Cache rules for anything the PPA
//! writes: the area is written back and invalidated first, so dirty CPU
//! lines never land on the PPA's result and the CPU never reads stale lines.

use embedded_graphics_core::{
    geometry::{Point, Size},
    primitives::Rectangle,
};
use kumiki_gfx::{FramebufferPanel, Rgb565Le};

use crate::{
    cache,
    dsi::{self, Dsi, FB_BYTES, HEIGHT, WIDTH},
    ppa::{self, Block},
};

/// Blocks narrower than this (native pixels) are drawn by the CPU: the cache
/// maintenance before a PPA write covers whole rows, so its cost follows the
/// height, not the area.
const MIN_PPA_WIDTH: u32 = 64;

/// Frames a retired buffer may still be read by the DMA (see [`Dsi::show`]).
const RETIRE_FRAMES: u32 = 2;

/// The DSI panel with three framebuffers.
pub struct DsiPanel {
    dsi: Dsi,
    fbs: [usize; 3],
    front: usize,
    back: usize,
    /// Frame count when each buffer stopped being the front one.
    retired_at: [u32; 3],
    /// Area changed by the previous present.
    prev_changed: Option<Rectangle>,
    /// Statistics: presents, and how many of them had to wait.
    presents: u32,
    waits: u32,
}

impl DsiPanel {
    /// Wraps a running [`Dsi`] that scans `fbs[0]`; `fbs[1]` becomes the
    /// back buffer. All three hold the same picture (e.g. cleared), are
    /// [`FB_BYTES`] long, 8-byte aligned, in PSRAM, and are used by nothing
    /// else.
    pub fn new(dsi: Dsi, fbs: [usize; 3]) -> Self {
        let f = dsi.frames().wrapping_sub(RETIRE_FRAMES);
        DsiPanel { dsi, fbs, front: 0, back: 1, retired_at: [f; 3], prev_changed: None, presents: 0, waits: 0 }
    }

    /// The display (frame counter, DCS commands, ...).
    pub fn dsi_mut(&mut self) -> &mut Dsi {
        &mut self.dsi
    }

    /// Keeps the frame DMA going; call it at least every ~200 ms when not
    /// presenting (presenting calls it).
    pub fn service(&mut self) {
        self.dsi.service();
    }

    /// (presents, presents that waited for a buffer to leave the scan).
    pub fn stats(&self) -> (u32, u32) {
        (self.presents, self.waits)
    }

    fn block(&self, fb: usize, r: &Rectangle) -> Block {
        Block { base: fb, width: WIDTH as u32, height: HEIGHT as u32, x: r.top_left.x as u32, y: r.top_left.y as u32 }
    }

    /// Byte range of the rows of `r` in `fb` (cache maintenance works on lines).
    fn rows(fb: usize, r: &Rectangle) -> (usize, usize) {
        let y0 = r.top_left.y as usize;
        let h = r.size.height as usize;
        (fb + y0 * WIDTH * 2, h * WIDTH * 2)
    }

    /// Prepares `r` in `fb` for a PPA write: written back, then dropped.
    fn before_ppa_write(fb: usize, r: &Rectangle) {
        let (a, n) = Self::rows(fb, r);
        dsi::flush(a, n);
        cache::invalidate(a & !127, ((a + n + 127) & !127) - (a & !127), true);
    }

    /// Copies `r` from `src` to `dst` (different buffers) and leaves it in
    /// memory for the DMA.
    fn copy_area(&self, dst: usize, src: usize, r: &Rectangle) -> Result<(), PpaError> {
        if r.size.width >= MIN_PPA_WIDTH {
            Self::before_ppa_write(dst, r);
            return ppa::copy(&self.block(dst, r), &self.block(src, r), r.size.width, r.size.height).map_err(PpaError);
        }
        let (x, w) = (r.top_left.x as usize * 2, r.size.width as usize * 2);
        for y in r.top_left.y as usize..(r.top_left.y as usize + r.size.height as usize) {
            let off = y * WIDTH * 2 + x;
            // SAFETY: both buffers are FB_BYTES long and owned by the panel;
            // `r` is inside the frame.
            unsafe { core::ptr::copy_nonoverlapping((src + off) as *const u8, (dst + off) as *mut u8, w) };
        }
        let (a, n) = Self::rows(dst, r);
        dsi::flush(a, n);
        Ok(())
    }
}

fn union(a: Option<Rectangle>, b: &Rectangle) -> Rectangle {
    match a {
        None => *b,
        Some(a) => {
            let tl = Point::new(a.top_left.x.min(b.top_left.x), a.top_left.y.min(b.top_left.y));
            let br = Point::new(
                (a.top_left.x + a.size.width as i32).max(b.top_left.x + b.size.width as i32),
                (a.top_left.y + a.size.height as i32).max(b.top_left.y + b.size.height as i32),
            );
            Rectangle::new(tl, Size::new((br.x - tl.x) as u32, (br.y - tl.y) as u32))
        }
    }
}

/// Error of [`DsiPanel`]: a PPA failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PpaError(pub &'static str);

/// Drawing into the frame buffer cannot fail; lets it share this error type.
impl From<core::convert::Infallible> for PpaError {
    fn from(e: core::convert::Infallible) -> Self {
        match e {}
    }
}

impl FramebufferPanel for DsiPanel {
    type Format = Rgb565Le;
    type Error = PpaError;

    fn size(&self) -> Size {
        Size::new(WIDTH as u32, HEIGHT as u32)
    }

    fn back_buffer(&mut self) -> &mut [u8] {
        // The buffers live in PSRAM and belong to this panel alone.
        unsafe { core::slice::from_raw_parts_mut(self.fbs[self.back] as *mut u8, FB_BYTES) }
    }

    fn flush_cache(&mut self, area: &Rectangle) {
        let (a, n) = Self::rows(self.fbs[self.back], area);
        dsi::flush(a, n);
    }

    fn present(&mut self, changed: Option<&Rectangle>) -> Result<(), PpaError> {
        let Some(changed) = changed else {
            self.dsi.service();
            return Ok(());
        };
        self.dsi.service();
        let (old_front, shown) = (self.front, self.back);
        self.dsi.show(self.fbs[shown]);
        self.retired_at[old_front] = self.dsi.frames();
        self.front = shown;
        // The third buffer, retired by the previous present.
        let next = 3 - old_front - shown;
        if self.dsi.frames().wrapping_sub(self.retired_at[next]) < RETIRE_FRAMES {
            self.waits += 1;
            // Two frames are ~31 ms; give up after 200 ms (the scan-out stopped).
            let end = crate::time::now() + 200_000 * crate::time::TICKS_PER_US;
            while self.dsi.frames().wrapping_sub(self.retired_at[next]) < RETIRE_FRAMES {
                self.dsi.service();
                if crate::time::now() > end {
                    return Err(PpaError("display scan-out stalled"));
                }
            }
        }
        self.presents += 1;
        // `next` holds the frame from two presents ago: bring the last two
        // presents' changes over from the frame now shown.
        let missing = union(self.prev_changed, changed);
        self.prev_changed = Some(*changed);
        self.back = next;
        self.copy_area(self.fbs[next], self.fbs[shown], &missing)
    }

    fn hw_fill(&mut self, area: &Rectangle, pixel: &[u8]) -> bool {
        if area.size.width < MIN_PPA_WIDTH {
            return false;
        }
        let fb = self.fbs[self.back];
        Self::before_ppa_write(fb, area);
        let color = u16::from_le_bytes([pixel[0], pixel[1]]);
        ppa::fill(&self.block(fb, area), area.size.width, area.size.height, color).is_ok()
    }

    fn hw_copy(&mut self, src: &Rectangle, dst_top_left: Point) -> bool {
        let dst = Rectangle::new(dst_top_left, src.size);
        if src.intersection(&dst).size != Size::zero() || src.size.width < MIN_PPA_WIDTH {
            // Overlapping: the PPA reads and writes in strips (tab5-os doc/44:
            // a rightward overlapping shift is slower than the CPU). Narrow:
            // see MIN_PPA_WIDTH.
            return false;
        }
        let fb = self.fbs[self.back];
        let (a, n) = Self::rows(fb, src);
        dsi::flush(a, n);
        Self::before_ppa_write(fb, &dst);
        ppa::copy(&self.block(fb, &dst), &self.block(fb, src), src.size.width, src.size.height).is_ok()
    }

    fn hw_threshold(&self) -> u32 {
        64 * 64
    }
}
