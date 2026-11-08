//! [`DsiPanel`]: the DSI output as a kumiki-gfx
//! [`FramebufferPanel`](kumiki_gfx::FramebufferPanel) (feature `gfx`).
//!
//! Double-buffered: the CPU (or the PPA) draws into the back buffer while the
//! DMA scans the front one. `present` points the DMA at the back buffer,
//! waits until the old front is no longer scanned (two frames, see
//! [`Dsi::show`]) and copies the changed area over with the PPA, so the new
//! back buffer again holds the frame on screen.
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

/// The DSI panel with two framebuffers.
pub struct DsiPanel {
    dsi: Dsi,
    fbs: [usize; 2],
    back: usize,
    swap_frame: u32,
}

impl DsiPanel {
    /// Wraps a running [`Dsi`] that scans `fbs[0]`; `fbs[1]` becomes the
    /// back buffer. Both are [`FB_BYTES`] long, 8-byte aligned, in PSRAM, and
    /// must not be used by anything else.
    pub fn new(dsi: Dsi, fbs: [usize; 2]) -> Self {
        let swap_frame = dsi.frames();
        DsiPanel { dsi, fbs, back: 1, swap_frame }
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

    fn wait_frames(&mut self, n: u32) {
        while self.dsi.frames().wrapping_sub(self.swap_frame) < n {
            self.dsi.service();
        }
    }
}

/// Error of [`DsiPanel`]: a PPA failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PpaError(pub &'static str);

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
        // (The previous present already waited until its old front was no
        // longer scanned, so the back buffer is free to show now.)
        let (front, back) = (self.fbs[1 - self.back], self.fbs[self.back]);
        self.dsi.show(back);
        self.swap_frame = self.dsi.frames();
        self.back = 1 - self.back;
        // The new back (old front) lacks this frame's changes: copy them
        // from the new front once the DMA no longer reads the new back.
        self.wait_frames(2);
        Self::before_ppa_write(front, changed);
        let (w, h) = (changed.size.width, changed.size.height);
        ppa::copy(&self.block(front, changed), &self.block(back, changed), w, h).map_err(PpaError)
    }

    fn hw_fill(&mut self, area: &Rectangle, pixel: &[u8]) -> bool {
        let fb = self.fbs[self.back];
        Self::before_ppa_write(fb, area);
        let color = u16::from_le_bytes([pixel[0], pixel[1]]);
        ppa::fill(&self.block(fb, area), area.size.width, area.size.height, color).is_ok()
    }

    fn hw_copy(&mut self, src: &Rectangle, dst_top_left: Point) -> bool {
        let dst = Rectangle::new(dst_top_left, src.size);
        if src.intersection(&dst).size != Size::zero() {
            return false; // overlapping: the PPA reads and writes in strips
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
