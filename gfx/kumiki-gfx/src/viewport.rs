//! [`Viewport`]: a translated, clipped window onto another draw target.
//!
//! Typical use: the *safe area* of a round panel (the square inscribed in the
//! visible circle), so code written for rectangular screens stays visible.

use embedded_graphics_core::{
    draw_target::DrawTarget,
    geometry::{OriginDimensions, Point, Size},
    primitives::Rectangle,
    Pixel,
};

use crate::{
    draw::{clip, FastDraw},
    pixel::PixelFormat,
};

/// A window of `area` on `target`, with its own origin at `area.top_left`.
pub struct Viewport<'a, D> {
    target: &'a mut D,
    area: Rectangle,
}

impl<'a, D: DrawTarget> Viewport<'a, D> {
    /// Create a viewport. `area` is clipped to the target's bounding box.
    pub fn new(target: &'a mut D, area: Rectangle) -> Self {
        let area = area.intersection(&target.bounding_box());
        Self { target, area }
    }

    /// The window in target coordinates.
    pub fn area(&self) -> Rectangle {
        self.area
    }

    fn local_bounds(&self) -> Rectangle {
        Rectangle::new(Point::zero(), self.area.size)
    }
}

impl<D: DrawTarget> OriginDimensions for Viewport<'_, D> {
    fn size(&self) -> Size {
        self.area.size
    }
}

impl<D: DrawTarget> DrawTarget for Viewport<'_, D> {
    type Color = D::Color;
    type Error = D::Error;

    fn draw_iter<I: IntoIterator<Item = Pixel<D::Color>>>(&mut self, pixels: I) -> Result<(), D::Error> {
        let (w, h) = (self.area.size.width as i32, self.area.size.height as i32);
        let off = self.area.top_left;
        self.target.draw_iter(
            pixels
                .into_iter()
                .filter(|Pixel(p, _)| p.x >= 0 && p.y >= 0 && p.x < w && p.y < h)
                .map(|Pixel(p, c)| Pixel(p + off, c)),
        )
    }

    fn fill_contiguous<I: IntoIterator<Item = D::Color>>(&mut self, area: &Rectangle, colors: I) -> Result<(), D::Error> {
        let Some(c) = clip(area, self.area.size) else { return Ok(()) };
        let aw = area.size.width as i32;
        let (cx0, cy0) = (c.src_offset.x, c.src_offset.y);
        let (cw, ch) = (c.dst.size.width as i32, c.dst.size.height as i32);
        let inside = colors.into_iter().enumerate().filter_map(move |(i, col)| {
            let (x, y) = (i as i32 % aw, i as i32 / aw);
            (x >= cx0 && x < cx0 + cw && y >= cy0 && y < cy0 + ch).then_some(col)
        });
        let dst = Rectangle::new(c.dst.top_left + self.area.top_left, c.dst.size);
        self.target.fill_contiguous(&dst, inside)
    }

    fn fill_solid(&mut self, area: &Rectangle, color: D::Color) -> Result<(), D::Error> {
        let r = area.intersection(&self.local_bounds());
        if r.size.width == 0 || r.size.height == 0 {
            return Ok(());
        }
        self.target.fill_solid(&Rectangle::new(r.top_left + self.area.top_left, r.size), color)
    }

    fn clear(&mut self, color: D::Color) -> Result<(), D::Error> {
        let a = self.area;
        self.target.fill_solid(&a, color)
    }
}

impl<D: FastDraw> FastDraw for Viewport<'_, D> {
    type Format = D::Format;

    fn fill_rect(&mut self, area: &Rectangle, color: D::Color) -> Result<(), D::Error> {
        self.fill_solid(area, color)
    }

    fn blit<S: PixelFormat>(&mut self, area: &Rectangle, src: &[u8], stride_px: usize) -> Result<(), D::Error> {
        let Some(c) = clip(area, self.area.size) else { return Ok(()) };
        // Skip the clipped-away rows/columns of the source, then blit the rest.
        let off = (c.src_offset.y as usize * stride_px + c.src_offset.x as usize) * S::BYTES;
        let dst = Rectangle::new(c.dst.top_left + self.area.top_left, c.dst.size);
        self.target.blit::<S>(&dst, &src[off..], stride_px)
    }
}
