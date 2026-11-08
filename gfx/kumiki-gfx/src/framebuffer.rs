//! Frame-buffer (scan-out) panels: [`FramebufferPanel`] and [`FbDisplay`].
//!
//! Panels such as MIPI-DSI video mode, DPI / RGB parallel or LCD_CAM RGB are
//! not written through a window command stream: a DMA engine reads whole
//! frames from memory, continuously. Drawing is then a memory write, and the
//! panel's job is to make those writes visible:
//!
//! 1. the CPU draws into the **back buffer** ([`FramebufferPanel::back_buffer`]);
//! 2. [`FramebufferPanel::flush_cache`] writes the CPU cache back for the
//!    changed area, so the scan-out DMA sees it;
//! 3. [`FramebufferPanel::present`] shows it: a no-op for a single buffer
//!    (the DMA already scans it), a swap at the next frame boundary for a
//!    double buffer.
//!
//! A panel may also offer a 2D engine (ESP32-P4 PPA, DMA2D, ...) for large
//! fills and copies ([`FramebufferPanel::hw_fill`], [`FramebufferPanel::hw_copy`]).
//!
//! [`FbDisplay`] puts embedded-graphics' `DrawTarget` and [`FastDraw`] on top
//! of such a panel, tracks the changed area for `flush`, and rotates in
//! software ([`FbDisplay::set_rotation`]): scan-out panels have no MADCTL.

use embedded_graphics_core::{
    draw_target::DrawTarget,
    geometry::{OriginDimensions, Point, Size},
    primitives::Rectangle,
    Pixel,
};

use crate::{
    draw::{clip, FastDraw},
    panel::Rotation,
    pixel::{self, PixelFormat},
    sprite::Sprite,
};

/// A panel that scans frames out of memory.
pub trait FramebufferPanel {
    /// Pixel layout of the frame buffer.
    type Format: PixelFormat;
    /// Error type.
    type Error;

    /// Frame size in pixels (the buffer is `width * height` pixels, no padding).
    fn size(&self) -> Size;

    /// The buffer the CPU draws into.
    fn back_buffer(&mut self) -> &mut [u8];

    /// Make CPU writes inside `area` of the back buffer visible to the
    /// scan-out DMA (cache write-back). No-op for uncached buffers.
    fn flush_cache(&mut self, area: &Rectangle);

    /// Show the back buffer. `changed` is the area drawn since the last
    /// present (`None`: nothing). Afterwards the back buffer must again hold
    /// the frame now shown, so drawing can continue incrementally: a
    /// single-buffered panel does nothing; a double-buffered one swaps at the
    /// next frame boundary and brings `changed` over into the new back buffer.
    fn present(&mut self, changed: Option<&Rectangle>) -> Result<(), Self::Error>;

    /// Fill `area` (inside the frame) of the back buffer with one pixel
    /// (`pixel` is `Format::BYTES` long) using a 2D engine. Returns `false`
    /// if not supported, in which case the caller draws it with the CPU.
    ///
    /// The implementation is responsible for cache coherence: CPU writes in
    /// `area` still in the cache must not later overwrite the engine's
    /// result, and the CPU must not read stale lines afterwards.
    fn hw_fill(&mut self, _area: &Rectangle, _pixel: &[u8]) -> bool {
        false
    }

    /// Copy `src` (inside the frame) to `dst_top_left` within the back buffer
    /// using a 2D engine; same contract as [`hw_fill`](Self::hw_fill).
    fn hw_copy(&mut self, _src: &Rectangle, _dst_top_left: Point) -> bool {
        false
    }

    /// Smallest area (in pixels) worth handing to the 2D engine.
    fn hw_threshold(&self) -> u32 {
        u32::MAX
    }
}

/// embedded-graphics drawing on a [`FramebufferPanel`], optionally rotated.
///
/// Coordinates are *logical* (after rotation); the dirty area and every
/// panel call are in the panel's *native* coordinates.
pub struct FbDisplay<P: FramebufferPanel> {
    panel: P,
    rotation: Rotation,
    dirty: Option<(i32, i32, i32, i32)>, // native x0, y0, x1, y1 inclusive
}

impl<P: FramebufferPanel> FbDisplay<P> {
    /// Wrap `panel` (not rotated). The whole frame starts out dirty.
    pub fn new(panel: P) -> Self {
        let s = panel.size();
        Self { panel, rotation: Rotation::Deg0, dirty: Some((0, 0, s.width as i32 - 1, s.height as i32 - 1)) }
    }

    /// Builder form of [`set_rotation`](Self::set_rotation).
    pub fn with_rotation(mut self, r: Rotation) -> Self {
        self.rotation = r;
        self
    }

    /// Turn the logical image clockwise by `r` relative to the panel's native
    /// orientation. What is already in the frame buffer stays as it is.
    pub fn set_rotation(&mut self, r: Rotation) {
        self.rotation = r;
    }

    /// The current rotation.
    pub fn rotation(&self) -> Rotation {
        self.rotation
    }

    /// A point in native panel coordinates (e.g. a touch report) in logical
    /// coordinates.
    pub fn native_to_logical(&self, p: Point) -> Point {
        self.rotation.to_logical(p, self.panel.size())
    }

    /// The panel.
    pub fn panel_mut(&mut self) -> &mut P {
        &mut self.panel
    }

    /// Release the panel.
    pub fn into_panel(self) -> P {
        self.panel
    }

    fn native(&self) -> Size {
        self.panel.size()
    }

    fn surface(&mut self) -> Sprite<P::Format, &mut [u8]> {
        let s = self.panel.size();
        Sprite::new(self.panel.back_buffer(), s.width as u16, s.height as u16)
            .expect("FramebufferPanel::back_buffer is smaller than size()")
    }

    fn logical_bounds(&self) -> Rectangle {
        Rectangle::new(Point::zero(), self.rotation.logical_size(self.native()))
    }

    /// Native rectangle of logical `r`, clipped.
    fn to_native(&self, r: &Rectangle) -> Rectangle {
        let r = r.intersection(&self.logical_bounds());
        self.rotation.rect_to_native(&r, self.native())
    }

    /// Grow the dirty area by native rectangle `r`.
    fn mark(&mut self, r: &Rectangle) {
        if r.size.width == 0 || r.size.height == 0 {
            return;
        }
        let (x0, y0) = (r.top_left.x, r.top_left.y);
        let (x1, y1) = (x0 + r.size.width as i32 - 1, y0 + r.size.height as i32 - 1);
        self.dirty = Some(match self.dirty {
            None => (x0, y0, x1, y1),
            Some((a, b, c, d)) => (a.min(x0), b.min(y0), c.max(x1), d.max(y1)),
        });
    }

    /// Mark the whole frame changed: the next [`flush`](Self::flush) writes
    /// it all back and presents it (e.g. after the platform replaced the
    /// buffer's content, as an OS taking a background app's frame away).
    pub fn invalidate(&mut self) {
        let s = self.native();
        self.dirty = Some((0, 0, s.width as i32 - 1, s.height as i32 - 1));
    }

    /// The area changed since the last [`flush`](Self::flush), in native
    /// panel coordinates.
    pub fn dirty(&self) -> Option<Rectangle> {
        self.dirty.map(|(a, b, c, d)| Rectangle::new(Point::new(a, b), Size::new((c - a + 1) as u32, (d - b + 1) as u32)))
    }

    /// Write the changed area back from the cache and present the frame.
    pub fn flush(&mut self) -> Result<(), P::Error> {
        let dirty = self.dirty();
        if let Some(r) = &dirty {
            self.panel.flush_cache(r);
        }
        self.dirty = None;
        self.panel.present(dirty.as_ref())
    }

    /// Copy a block within the frame (logical `src` to `dst_top_left`), with
    /// the 2D engine when the panel has one and the block is large enough.
    pub fn copy_within(&mut self, src: &Rectangle, dst_top_left: Point) {
        let b = self.logical_bounds();
        let src = src.intersection(&b);
        let dst = Rectangle::new(dst_top_left, src.size).intersection(&b);
        if dst.size.width == 0 || dst.size.height == 0 {
            return;
        }
        let src = Rectangle::new(src.top_left + (dst.top_left - dst_top_left), dst.size);
        // A rotation maps the copy to a copy between the native rectangles.
        let (src, dst) = (self.to_native(&src), self.to_native(&dst));
        let area = dst.size.width * dst.size.height;
        if area >= self.panel.hw_threshold() && self.panel.hw_copy(&src, dst.top_left) {
            self.mark(&dst);
            return;
        }
        let w = self.native().width as usize;
        let bpp = P::Format::BYTES;
        let row = dst.size.width as usize * bpp;
        let buf = self.panel.back_buffer();
        let rows: &mut dyn Iterator<Item = u32> = if dst.top_left.y > src.top_left.y {
            &mut (0..dst.size.height).rev()
        } else {
            &mut (0..dst.size.height)
        };
        for r in rows {
            let s = ((src.top_left.y as usize + r as usize) * w + src.top_left.x as usize) * bpp;
            let d = ((dst.top_left.y as usize + r as usize) * w + dst.top_left.x as usize) * bpp;
            buf.copy_within(s..s + row, d);
        }
        self.mark(&dst);
    }
}

impl<P: FramebufferPanel> OriginDimensions for FbDisplay<P> {
    fn size(&self) -> Size {
        self.rotation.logical_size(self.native())
    }
}

impl<P: FramebufferPanel> DrawTarget for FbDisplay<P> {
    type Color = <P::Format as PixelFormat>::Color;
    type Error = core::convert::Infallible;

    fn draw_iter<I: IntoIterator<Item = Pixel<Self::Color>>>(&mut self, pixels: I) -> Result<(), Self::Error> {
        let logical = self.logical_bounds().size;
        let (lw, lh) = (logical.width as i32, logical.height as i32);
        let (rot, native) = (self.rotation, self.native());
        let mut bb: Option<(i32, i32, i32, i32)> = None;
        self.surface().draw_iter(pixels.into_iter().filter_map(|Pixel(p, c)| {
            if p.x < 0 || p.y < 0 || p.x >= lw || p.y >= lh {
                return None;
            }
            let q = rot.to_native(p, native);
            bb = Some(match bb {
                None => (q.x, q.y, q.x, q.y),
                Some((a, b, c, d)) => (a.min(q.x), b.min(q.y), c.max(q.x), d.max(q.y)),
            });
            Some(Pixel(q, c))
        }))?;
        if let Some((a, b, c, d)) = bb {
            self.mark(&Rectangle::new(Point::new(a, b), Size::new((c - a + 1) as u32, (d - b + 1) as u32)));
        }
        Ok(())
    }

    fn fill_contiguous<I: IntoIterator<Item = Self::Color>>(&mut self, area: &Rectangle, colors: I) -> Result<(), Self::Error> {
        if self.rotation == Rotation::Deg0 {
            let n = self.to_native(area);
            self.surface().fill_contiguous(area, colors)?;
            self.mark(&n);
            return Ok(());
        }
        let w = area.size.width.max(1) as i32;
        let tl = area.top_left;
        self.draw_iter(
            colors
                .into_iter()
                .take((area.size.width * area.size.height) as usize)
                .enumerate()
                .map(|(i, c)| Pixel(tl + Point::new(i as i32 % w, i as i32 / w), c)),
        )
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        FastDraw::fill_rect(self, area, color)
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        let all = self.logical_bounds();
        FastDraw::fill_rect(self, &all, color)
    }
}

impl<P: FramebufferPanel> FastDraw for FbDisplay<P> {
    type Format = P::Format;

    fn fill_rect(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let a = self.to_native(area);
        if a.size.width == 0 || a.size.height == 0 {
            return Ok(());
        }
        let mut px = [0u8; 4];
        P::Format::encode(color, &mut px);
        let hw = a.size.width * a.size.height >= self.panel.hw_threshold()
            && self.panel.hw_fill(&a, &px[..P::Format::BYTES]);
        if !hw {
            FastDraw::fill_rect(&mut self.surface(), &a, color)?;
        }
        self.mark(&a);
        Ok(())
    }

    fn blit<S: PixelFormat>(&mut self, area: &Rectangle, src: &[u8], stride_px: usize) -> Result<(), Self::Error> {
        if self.rotation == Rotation::Deg0 {
            let n = self.to_native(area);
            self.surface().blit::<S>(area, src, stride_px)?;
            self.mark(&n);
            return Ok(());
        }
        let Some(c) = clip(area, self.logical_bounds().size) else { return Ok(()) };
        let (rot, native) = (self.rotation, self.native());
        let nw = native.width as usize;
        let bpp = P::Format::BYTES;
        let buf = self.panel.back_buffer();
        for y in 0..c.dst.size.height as i32 {
            let sy = (c.src_offset.y + y) as usize;
            for x in 0..c.dst.size.width as i32 {
                let sx = (c.src_offset.x + x) as usize;
                let s = (sy * stride_px + sx) * S::BYTES;
                let q = rot.to_native(c.dst.top_left + Point::new(x, y), native);
                let d = (q.y as usize * nw + q.x as usize) * bpp;
                pixel::convert::<S, P::Format>(&src[s..s + S::BYTES], &mut buf[d..d + bpp], 1);
            }
        }
        let n = rot.rect_to_native(&c.dst, native);
        self.mark(&n);
        Ok(())
    }
}
