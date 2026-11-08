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
