//! [`Sprite`]: an in-RAM drawing surface stored in a panel's wire format.

use core::{convert::Infallible, marker::PhantomData};

use embedded_graphics_core::{
    draw_target::DrawTarget,
    geometry::{OriginDimensions, Point, Size},
    primitives::Rectangle,
    Pixel,
};

use crate::{
    draw::{clip, required_src_len, AsyncFastDraw, BlitStream, FastDraw},
    pixel::{self, PixelFormat},
};

/// An off-screen surface of `width × height` pixels in format `F`, backed by
/// any byte buffer (`&mut [u8]`, `[u8; N]`, `Vec<u8>`, a DMA-capable region, ...).
///
/// Because pixels are stored in wire order, [`Sprite::push`] to a panel of the
/// same format is a plain copy into the bus' DMA buffers.
pub struct Sprite<F: PixelFormat, B> {
    buf: B,
    width: u16,
    height: u16,
    _f: PhantomData<F>,
}

impl<F: PixelFormat, B: AsRef<[u8]> + AsMut<[u8]>> Sprite<F, B> {
    /// Bytes needed for a `width × height` sprite.
    pub const fn bytes_for(width: u16, height: u16) -> usize {
        width as usize * height as usize * F::BYTES
    }

    /// Create a sprite over `buf`. Returns `None` if `buf` is too small.
    pub fn new(buf: B, width: u16, height: u16) -> Option<Self> {
        (buf.as_ref().len() >= Self::bytes_for(width, height)).then_some(Self { buf, width, height, _f: PhantomData })
    }

    /// Pixel data in wire order, row-major, no padding.
    pub fn bytes(&self) -> &[u8] {
        &self.buf.as_ref()[..Self::bytes_for(self.width, self.height)]
    }

    /// Mutable pixel data.
    pub fn bytes_mut(&mut self) -> &mut [u8] {
        let n = Self::bytes_for(self.width, self.height);
        &mut self.buf.as_mut()[..n]
    }

    /// Release the backing buffer.
    pub fn into_inner(self) -> B {
        self.buf
    }

    fn rect_at(&self, pos: Point) -> Rectangle {
        Rectangle::new(pos, Size::new(self.width as u32, self.height as u32))
    }

    /// Push the whole sprite to `target` with its top-left corner at `pos`.
    pub fn push<T: FastDraw>(&self, target: &mut T, pos: Point) -> Result<(), T::Error> {
        target.blit::<F>(&self.rect_at(pos), self.bytes(), self.width as usize)
    }

    /// `async` version of [`Sprite::push`].
    pub async fn push_async<T: AsyncFastDraw>(&self, target: &mut T, pos: Point) -> Result<(), T::Error> {
        target.blit::<F>(&self.rect_at(pos), self.bytes(), self.width as usize).await
    }

    /// Push skipping pixels equal to `key` (colour-key transparency). Each
    /// opaque horizontal run becomes one window write, like M5GFX's `pushSprite(x, y, transp)`.
    pub fn push_keyed<T: FastDraw>(&self, target: &mut T, pos: Point, key: F::Color) -> Result<(), T::Error> {
        let (k, n) = pixel::encode_one::<F>(key);
        for (y, start, len) in self.opaque_runs(&k[..n]) {
            let off = (y * self.width as usize + start) * F::BYTES;
            let r = Rectangle::new(pos + Point::new(start as i32, y as i32), Size::new(len as u32, 1));
            target.blit::<F>(&r, &self.bytes()[off..off + len * F::BYTES], len)?;
        }
        Ok(())
    }

    /// `async` version of [`Sprite::push_keyed`].
    pub async fn push_keyed_async<T: AsyncFastDraw>(&self, target: &mut T, pos: Point, key: F::Color) -> Result<(), T::Error> {
        let (k, n) = pixel::encode_one::<F>(key);
        for (y, start, len) in self.opaque_runs(&k[..n]) {
            let off = (y * self.width as usize + start) * F::BYTES;
            let r = Rectangle::new(pos + Point::new(start as i32, y as i32), Size::new(len as u32, 1));
            target.blit::<F>(&r, &self.bytes()[off..off + len * F::BYTES], len).await?;
        }
        Ok(())
    }

    /// Push the sprite rotated by `angle_deg` (clockwise) and scaled by
    /// `zoom_x` / `zoom_y` (negative mirrors), with its centre at `center`
    /// on `target` — M5GFX's `pushRotateZoom`. Nearest-neighbour sampling;
    /// pixels equal to `key` (if given) are skipped. Each opaque horizontal
    /// run of the result is one blit.
    pub fn push_rotate_zoom<T: FastDraw>(
        &self,
        target: &mut T,
        center: Point,
        angle_deg: f32,
        zoom_x: f32,
        zoom_y: f32,
        key: Option<F::Color>,
    ) -> Result<(), T::Error> {
        if zoom_x == 0.0 || zoom_y == 0.0 || self.width == 0 || self.height == 0 {
            return Ok(());
        }
        let (s, c) = sin_cos_deg(angle_deg);
        let (w, h) = (self.width as f32, self.height as f32);
        let (cx, cy) = (center.x as f32, center.y as f32);
        // Destination bounding box: the four transformed corners.
        let (mut x0, mut y0, mut x1, mut y1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for (px, py) in [(-w / 2.0, -h / 2.0), (w / 2.0, -h / 2.0), (-w / 2.0, h / 2.0), (w / 2.0, h / 2.0)] {
            let (sx, sy) = (px * zoom_x, py * zoom_y);
            let (dx, dy) = (sx * c - sy * s + cx, sx * s + sy * c + cy);
            (x0, y0, x1, y1) = (x0.min(dx), y0.min(dy), x1.max(dx), y1.max(dy));
        }
        let bounds = target.bounding_box();
        let bx1 = bounds.top_left.x + bounds.size.width as i32;
        let by1 = bounds.top_left.y + bounds.size.height as i32;
        let (x0, x1) = ((x0 as i32 - 1).max(bounds.top_left.x), (x1 as i32 + 1).min(bx1));
        let (y0, y1) = ((y0 as i32 - 1).max(bounds.top_left.y), (y1 as i32 + 1).min(by1));
        if x0 >= x1 || y0 >= y1 {
            return Ok(());
        }
        // Inverse map in 16.16 fixed point: src = Z⁻¹·R(−θ)·(dst − centre) + pivot.
        const ONE: f32 = 65536.0;
        let du_dx = (c / zoom_x * ONE) as i32;
        let dv_dx = (-s / zoom_y * ONE) as i32;
        let (k, kn) = match key {
            Some(k) => {
                let (b, n) = pixel::encode_one::<F>(k);
                (b, n)
            }
            None => ([0; 4], 0),
        };
        let data = self.bytes();
        let (sw, sh) = (self.width as i32, self.height as i32);
        const RUN: usize = 128;
        let mut run = [0u8; RUN * 4];
        for y in y0..y1 {
            let dy = y as f32 + 0.5 - cy;
            let dx = x0 as f32 + 0.5 - cx;
            let mut u = (((c * dx + s * dy) / zoom_x + w / 2.0) * ONE) as i32;
            let mut v = (((-s * dx + c * dy) / zoom_y + h / 2.0) * ONE) as i32;
            let mut start = x0;
            let mut n = 0usize;
            for x in x0..x1 {
                let (iu, iv) = (u >> 16, v >> 16);
                let mut opaque = false;
                if u >= 0 && v >= 0 && iu < sw && iv < sh {
                    let o = (iv as usize * sw as usize + iu as usize) * F::BYTES;
                    let p = &data[o..o + F::BYTES];
                    if kn == 0 || p != &k[..kn] {
                        if n == 0 {
                            start = x;
                        }
                        run[n * F::BYTES..(n + 1) * F::BYTES].copy_from_slice(p);
                        n += 1;
                        opaque = true;
                    }
                }
                if (!opaque || n == RUN) && n > 0 {
                    target.blit::<F>(&Rectangle::new(Point::new(start, y), Size::new(n as u32, 1)), &run[..n * F::BYTES], n)?;
                    n = 0;
                }
                u += du_dx;
                v += dv_dx;
            }
            if n > 0 {
                target.blit::<F>(&Rectangle::new(Point::new(start, y), Size::new(n as u32, 1)), &run[..n * F::BYTES], n)?;
            }
        }
        Ok(())
    }

    /// Iterate `(row, start_col, len)` of runs whose pixels differ from `key`.
    fn opaque_runs<'a>(&'a self, key: &'a [u8]) -> impl Iterator<Item = (usize, usize, usize)> + 'a {
        let w = self.width as usize;
        let data = self.bytes();
        (0..self.height as usize).flat_map(move |y| {
            let row = &data[y * w * F::BYTES..(y + 1) * w * F::BYTES];
            let mut x = 0;
            core::iter::from_fn(move || {
                while x < w && &row[x * F::BYTES..(x + 1) * F::BYTES] == key {
                    x += 1;
                }
                if x >= w {
                    return None;
                }
                let start = x;
                while x < w && &row[x * F::BYTES..(x + 1) * F::BYTES] != key {
                    x += 1;
                }
                Some((y, start, x - start))
            })
        })
    }
}

impl<F: PixelFormat, B> OriginDimensions for Sprite<F, B> {
    fn size(&self) -> Size {
        Size::new(self.width as u32, self.height as u32)
    }
}

impl<F: PixelFormat, B: AsRef<[u8]> + AsMut<[u8]>> DrawTarget for Sprite<F, B> {
    type Color = F::Color;
    type Error = Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        let (w, h) = (self.width as i32, self.height as i32);
        let data = self.bytes_mut();
        for Pixel(p, c) in pixels {
            if p.x >= 0 && p.y >= 0 && p.x < w && p.y < h {
                let o = (p.y * w + p.x) as usize * F::BYTES;
                F::encode(c, &mut data[o..]);
            }
        }
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        FastDraw::fill_rect(self, area, color)
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        pixel::fill::<F>(self.bytes_mut(), color);
        Ok(())
    }
}

impl<F: PixelFormat, B: AsRef<[u8]> + AsMut<[u8]>> FastDraw for Sprite<F, B> {
    type Format = F;

    fn fill_rect(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let Some(c) = clip(area, self.size()) else { return Ok(()) };
        let w = self.width as usize;
        let (x0, y0) = (c.dst.top_left.x as usize, c.dst.top_left.y as usize);
        let (cw, ch) = (c.dst.size.width as usize, c.dst.size.height as usize);
        let data = self.bytes_mut();
        // Fill the first row, then replicate it (M5GFX's memset-then-memcpy strategy).
        let first = (y0 * w + x0) * F::BYTES;
        pixel::fill::<F>(&mut data[first..first + cw * F::BYTES], color);
        for y in 1..ch {
            let o = ((y0 + y) * w + x0) * F::BYTES;
            data.copy_within(first..first + cw * F::BYTES, o);
        }
        Ok(())
    }

    fn blit<S: PixelFormat>(&mut self, area: &Rectangle, src: &[u8], stride_px: usize) -> Result<(), Self::Error> {
        assert!(src.len() >= required_src_len::<S>(area.size, stride_px), "blit source too short");
        let Some(c) = clip(area, self.size()) else { return Ok(()) };
        let w = self.width as usize;
        let (x0, y0) = (c.dst.top_left.x as usize, c.dst.top_left.y as usize);
        let (cw, ch) = (c.dst.size.width as usize, c.dst.size.height as usize);
        let mut stream = BlitStream::<S, F>::new(src, stride_px, &c);
        let data = self.bytes_mut();
        for y in 0..ch {
            let o = ((y0 + y) * w + x0) * F::BYTES;
            stream.fill(&mut data[o..o + cw * F::BYTES]);
        }
        Ok(())
    }
}

/// `(sin, cos)` of `deg` degrees without libm (error < 1e-6).
pub(crate) fn sin_cos_deg(deg: f32) -> (f32, f32) {
    // Reduce to [-180, 180).
    let mut d = deg % 360.0;
    if d >= 180.0 {
        d -= 360.0;
    } else if d < -180.0 {
        d += 360.0;
    }
    // sin on [-90, 90] by an odd Taylor series; mirror the rest.
    let sin_red = |d: f32| {
        let x = d * (core::f32::consts::PI / 180.0);
        let x2 = x * x;
        x * (1.0 - x2 / 6.0 * (1.0 - x2 / 20.0 * (1.0 - x2 / 42.0 * (1.0 - x2 / 72.0 * (1.0 - x2 / 110.0)))))
    };
    let s = if d > 90.0 {
        sin_red(180.0 - d)
    } else if d < -90.0 {
        sin_red(-180.0 - d)
    } else {
        sin_red(d)
    };
    // cos(d) = sin(d + 90), reduced again.
    let mut e = d + 90.0;
    if e >= 180.0 {
        e -= 360.0;
    }
    let c = if e > 90.0 {
        sin_red(180.0 - e)
    } else if e < -90.0 {
        sin_red(-180.0 - e)
    } else {
        sin_red(e)
    };
    (s, c)
}
