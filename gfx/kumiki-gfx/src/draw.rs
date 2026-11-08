//! Fast drawing extension traits and shared helpers.
//!
//! [`FastDraw`] extends embedded-graphics' [`DrawTarget`] with operations that
//! keep the "contiguous buffer" information which the iterator-based
//! `DrawTarget` API loses: solid rectangle fills and raw buffer blits (with an
//! optional format conversion fused into the transfer).

use embedded_graphics_core::{
    draw_target::DrawTarget,
    geometry::{Point, Size},
    primitives::Rectangle,
};

use crate::pixel::{self, PixelFormat};

/// Fast-path drawing operations (blocking).
pub trait FastDraw: DrawTarget {
    /// Native wire format of this target.
    type Format: PixelFormat<Color = Self::Color>;

    /// Fill `area` (clipped to the target) with `color`.
    fn fill_rect(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error>;

    /// Copy a `area.size` block of pixels in format `S` from `src` to `area`
    /// (clipped to the target). `stride_px` is the source row pitch in pixels.
    fn blit<S: PixelFormat>(&mut self, area: &Rectangle, src: &[u8], stride_px: usize) -> Result<(), Self::Error>;
}

/// Fast-path drawing operations (`async`, executor-agnostic).
#[allow(async_fn_in_trait)]
pub trait AsyncFastDraw {
    /// Native wire format of this target.
    type Format: PixelFormat;
    /// Error type.
    type Error;

    /// Visible size.
    fn size(&self) -> Size;

    /// Fill `area` (clipped) with `color`.
    async fn fill_rect(
        &mut self,
        area: &Rectangle,
        color: <Self::Format as PixelFormat>::Color,
    ) -> Result<(), Self::Error>;

    /// Blit a block of `S` pixels (clipped). See [`FastDraw::blit`].
    async fn blit<S: PixelFormat>(&mut self, area: &Rectangle, src: &[u8], stride_px: usize) -> Result<(), Self::Error>;
}

/// Result of clipping a blit rectangle against a target.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clipped {
    /// Destination rectangle (inside the target, non-empty).
    pub dst: Rectangle,
    /// Offset of the first copied source pixel inside the source block.
    pub src_offset: Point,
}

/// Clip `area` against a target of `size`. Returns `None` if nothing is visible.
pub fn clip(area: &Rectangle, size: Size) -> Option<Clipped> {
    let bounds = Rectangle::new(Point::zero(), size);
    let dst = area.intersection(&bounds);
    if dst.size.width == 0 || dst.size.height == 0 {
        return None;
    }
    Some(Clipped { dst, src_offset: dst.top_left - area.top_left })
}

/// Streams a (clipped) source block row by row into arbitrarily sized output
/// chunks, converting `S` → `D` on the fly. Used as the `fill` closure body of
/// [`crate::bus::blocking::PanelBus::write_with`].
pub(crate) struct BlitStream<'a, S: PixelFormat, D: PixelFormat> {
    src: &'a [u8],
    stride_px: usize,
    x0: usize,
    width: usize,
    row: usize,
    col: usize,
    _f: core::marker::PhantomData<(S, D)>,
}

impl<'a, S: PixelFormat, D: PixelFormat> BlitStream<'a, S, D> {
    pub(crate) fn new(src: &'a [u8], stride_px: usize, c: &Clipped) -> Self {
        Self {
            src,
            stride_px,
            x0: c.src_offset.x as usize,
            width: c.dst.size.width as usize,
            row: c.src_offset.y as usize,
            col: 0,
            _f: core::marker::PhantomData,
        }
    }

    /// Fill `out` completely (its length is a multiple of `D::BYTES`).
    pub(crate) fn fill(&mut self, out: &mut [u8]) {
        let mut out_px = out.len() / D::BYTES;
        let mut o = 0;
        while out_px > 0 {
            let n = (self.width - self.col).min(out_px);
            let s = (self.row * self.stride_px + self.x0 + self.col) * S::BYTES;
            pixel::convert::<S, D>(&self.src[s..], &mut out[o..], n);
            o += n * D::BYTES;
            out_px -= n;
            self.col += n;
            if self.col == self.width {
                self.col = 0;
                self.row += 1;
            }
        }
    }
}

/// Minimum source length (bytes) for a `size` block with the given stride.
pub(crate) fn required_src_len<S: PixelFormat>(size: Size, stride_px: usize) -> usize {
    if size.height == 0 || size.width == 0 {
        return 0;
    }
    ((size.height as usize - 1) * stride_px + size.width as usize) * S::BYTES
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_offsets_source() {
        let c = clip(&Rectangle::new(Point::new(-2, -3), Size::new(10, 10)), Size::new(5, 5)).unwrap();
        assert_eq!(c.dst, Rectangle::new(Point::zero(), Size::new(5, 5)));
        assert_eq!(c.src_offset, Point::new(2, 3));
        assert!(clip(&Rectangle::new(Point::new(10, 0), Size::new(3, 3)), Size::new(5, 5)).is_none());
    }
}
