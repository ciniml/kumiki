//! [`BufferedDisplay`]: draw into a RAM frame buffer, push only what changed.
//!
//! Useful when
//! * the panel imposes window alignment (AMOLED controllers such as CO5300
//!   need even x / width), so arbitrary small writes cannot go to the panel;
//! * many small primitives would otherwise each cost a window setup;
//! * flicker-free composition is wanted.
//!
//! Drawing goes to the frame buffer and grows a dirty rectangle;
//! [`BufferedDisplay::flush`] (or `flush_async`) sends the dirty rectangle,
//! widened to the required alignment, in one blit.

use core::convert::Infallible;

use embedded_graphics_core::{
    draw_target::DrawTarget,
    geometry::{OriginDimensions, Point, Size},
    primitives::Rectangle,
    Pixel,
};

use crate::{
    draw::{AsyncFastDraw, FastDraw},
    pixel::PixelFormat,
    sprite::Sprite,
};

/// A frame-buffered display.
pub struct BufferedDisplay<D, F: PixelFormat, B> {
    target: D,
    fb: Sprite<F, B>,
    dirty: Option<(i32, i32, i32, i32)>, // x0, y0, x1, y1 inclusive
    align: i32,
}

impl<D, F: PixelFormat, B: AsRef<[u8]> + AsMut<[u8]>> BufferedDisplay<D, F, B> {
    /// Wrap `target` (of `size`) with a frame buffer in `buf`; flushes widen
    /// the dirty area so that x and width are multiples of `x_align`.
    /// Returns `None` if `buf` is too small.
    pub fn new(target: D, size: Size, buf: B, x_align: u16) -> Option<Self> {
        let fb = Sprite::new(buf, size.width as u16, size.height as u16)?;
        Some(Self { target, fb, dirty: Some((0, 0, size.width as i32 - 1, size.height as i32 - 1)), align: x_align.max(1) as i32 })
    }

    /// The underlying display.
    pub fn target_mut(&mut self) -> &mut D {
        &mut self.target
    }

    /// Release the underlying display (the frame buffer is dropped).
    pub fn into_target(self) -> D {
        self.target
    }

    /// Mark the whole screen for the next flush.
    pub fn invalidate(&mut self) {
        let s = self.fb.size();
        self.dirty = Some((0, 0, s.width as i32 - 1, s.height as i32 - 1));
    }

    fn mark(&mut self, r: &Rectangle) {
        let s = self.fb.size();
        let r = r.intersection(&Rectangle::new(Point::zero(), s));
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

    /// Take the dirty rectangle, aligned, with its frame-buffer byte offset.
    fn take_dirty(&mut self) -> Option<(Rectangle, usize)> {
        let (x0, y0, x1, y1) = self.dirty.take()?;
        let w = self.fb.size().width as i32;
        let x0 = x0 / self.align * self.align;
        let x1 = ((x1 + self.align) / self.align * self.align - 1).min(w - 1);
        let rect = Rectangle::new(Point::new(x0, y0), Size::new((x1 - x0 + 1) as u32, (y1 - y0 + 1) as u32));
        Some((rect, (y0 * w + x0) as usize * F::BYTES))
    }

    /// Send the dirty area to the display (blocking).
    pub fn flush(&mut self) -> Result<(), D::Error>
    where
        D: FastDraw,
    {
        if let Some((rect, off)) = self.take_dirty() {
            let stride = self.fb.size().width as usize;
            self.target.blit::<F>(&rect, &self.fb.bytes()[off..], stride)?;
        }
        Ok(())
    }

    /// Send the dirty area to the display (`async`).
    pub async fn flush_async(&mut self) -> Result<(), D::Error>
    where
        D: AsyncFastDraw,
    {
        if let Some((rect, off)) = self.take_dirty() {
            let stride = self.fb.size().width as usize;
            self.target.blit::<F>(&rect, &self.fb.bytes()[off..], stride).await?;
        }
        Ok(())
    }
}

impl<D, F: PixelFormat, B> OriginDimensions for BufferedDisplay<D, F, B> {
    fn size(&self) -> Size {
        self.fb.size()
    }
}

impl<D, F: PixelFormat, B: AsRef<[u8]> + AsMut<[u8]>> DrawTarget for BufferedDisplay<D, F, B> {
    type Color = F::Color;
    type Error = Infallible;

    fn draw_iter<I: IntoIterator<Item = Pixel<F::Color>>>(&mut self, pixels: I) -> Result<(), Infallible> {
        let s = self.fb.size();
        let (w, h) = (s.width as i32, s.height as i32);
        let mut bb: Option<(i32, i32, i32, i32)> = None;
        let fb = &mut self.fb;
        fb.draw_iter(pixels.into_iter().inspect(|Pixel(p, _)| {
            if p.x >= 0 && p.y >= 0 && p.x < w && p.y < h {
                bb = Some(match bb {
                    None => (p.x, p.y, p.x, p.y),
                    Some((a, b, c, d)) => (a.min(p.x), b.min(p.y), c.max(p.x), d.max(p.y)),
                });
            }
        }))?;
        if let Some((a, b, c, d)) = bb {
            self.mark(&Rectangle::new(Point::new(a, b), Size::new((c - a + 1) as u32, (d - b + 1) as u32)));
        }
        Ok(())
    }

    fn fill_contiguous<I: IntoIterator<Item = F::Color>>(&mut self, area: &Rectangle, colors: I) -> Result<(), Infallible> {
        self.fb.fill_contiguous(area, colors)?;
        self.mark(area);
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: F::Color) -> Result<(), Infallible> {
        FastDraw::fill_rect(&mut self.fb, area, color)?;
        self.mark(area);
        Ok(())
    }

    fn clear(&mut self, color: F::Color) -> Result<(), Infallible> {
        self.fb.clear(color)?;
        self.invalidate();
        Ok(())
    }
}

impl<D, F: PixelFormat, B: AsRef<[u8]> + AsMut<[u8]>> FastDraw for BufferedDisplay<D, F, B> {
    type Format = F;

    fn fill_rect(&mut self, area: &Rectangle, color: F::Color) -> Result<(), Infallible> {
        self.fill_solid(area, color)
    }

    fn blit<S: PixelFormat>(&mut self, area: &Rectangle, src: &[u8], stride_px: usize) -> Result<(), Infallible> {
        self.fb.blit::<S>(area, src, stride_px)?;
        self.mark(area);
        Ok(())
    }
}
