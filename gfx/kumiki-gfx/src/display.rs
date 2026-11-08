//! [`Display`]: a drawing surface that writes straight to a window panel.

use embedded_graphics_core::{
    draw_target::DrawTarget,
    geometry::{OriginDimensions, Point, Size},
    primitives::Rectangle,
    Pixel,
};

use crate::{
    bus::{asynch::PanelBus as AsyncPanelBus, blocking::PanelBus, ErrorType},
    draw::{clip, required_src_len, AsyncFastDraw, BlitStream, FastDraw},
    panel::{asynch::WindowPanel as AsyncWindowPanel, blocking::WindowPanel, Panel},
    pixel::{self, PixelFormat},
};

/// Bus error of a blocking window panel.
pub type BusError<P> = <<P as WindowPanel>::Bus as ErrorType>::Error;
/// Bus error of an `async` window panel.
pub type AsyncBusError<P> = <<P as AsyncWindowPanel>::Bus as ErrorType>::Error;

/// Maximum pixels gathered into one horizontal run by `draw_iter`.
const RUN_PX: usize = 64;

/// A display surface backed by a window panel.
///
/// * With a blocking panel it implements embedded-graphics' [`DrawTarget`]
///   (with fast `fill_solid` / `fill_contiguous` / `clear`) and [`FastDraw`].
/// * With an `async` panel it implements [`AsyncFastDraw`]; compose in RAM
///   (e.g. a [`crate::Sprite`]) and push asynchronously.
pub struct Display<P> {
    panel: P,
}

impl<P: Panel> Display<P> {
    /// Wrap an initialized panel.
    pub fn new(panel: P) -> Self {
        Self { panel }
    }
    /// Access the panel (e.g. to change rotation or brightness).
    pub fn panel(&mut self) -> &mut P {
        &mut self.panel
    }
    /// Unwrap.
    pub fn into_panel(self) -> P {
        self.panel
    }
}

impl<P: Panel> OriginDimensions for Display<P> {
    fn size(&self) -> Size {
        self.panel.size()
    }
}

// ---------------------------------------------------------------------------
// Blocking
// ---------------------------------------------------------------------------

impl<P: WindowPanel> Display<P> {
    fn flush_run(&mut self, start: Point, len: usize, buf: &[u8]) -> Result<(), BusError<P>> {
        if len == 0 {
            return Ok(());
        }
        let area = Rectangle::new(start, Size::new(len as u32, 1));
        self.panel.begin_write(&area)?;
        self.panel.bus().write_data(&buf[..len * P::Format::BYTES])
    }
}

impl<P: WindowPanel> DrawTarget for Display<P> {
    type Color = <P::Format as PixelFormat>::Color;
    type Error = BusError<P>;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        // Gather horizontally adjacent pixels into runs (one window per run).
        let size = self.size();
        let mut buf = [0u8; RUN_PX * 4];
        let mut start = Point::zero();
        let mut len = 0usize;
        for Pixel(p, c) in pixels {
            if p.x < 0 || p.y < 0 || p.x >= size.width as i32 || p.y >= size.height as i32 {
                continue;
            }
            let contiguous = len > 0 && len < RUN_PX && p.y == start.y && p.x == start.x + len as i32;
            if !contiguous {
                self.flush_run(start, len, &buf)?;
                start = p;
                len = 0;
            }
            P::Format::encode(c, &mut buf[len * P::Format::BYTES..]);
            len += 1;
        }
        self.flush_run(start, len, &buf)
    }

    fn fill_contiguous<I>(&mut self, area: &Rectangle, colors: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Self::Color>,
    {
        let Some(c) = clip(area, self.size()) else { return Ok(()) };
        self.panel.begin_write(&c.dst)?;
        // Stream only the colours that fall inside the clipped rectangle; in
        // row-major order they form exactly `c.dst` row-major.
        let aw = area.size.width as i32;
        let (cx0, cy0) = (c.src_offset.x, c.src_offset.y);
        let (cw, ch) = (c.dst.size.width as i32, c.dst.size.height as i32);
        let mut it = colors.into_iter().enumerate().filter_map(move |(i, col)| {
            let (x, y) = (i as i32 % aw, i as i32 / aw);
            (x >= cx0 && x < cx0 + cw && y >= cy0 && y < cy0 + ch).then_some(col)
        });
        let total = (cw * ch) as usize * P::Format::BYTES;
        let mut exhausted = false;
        self.panel.bus().write_with(total, P::Format::BYTES, |buf| {
            for px in buf.chunks_exact_mut(P::Format::BYTES) {
                match it.next() {
                    Some(col) => P::Format::encode(col, px),
                    None => {
                        exhausted = true;
                        px.fill(0)
                    }
                }
            }
        })?;
        let _ = exhausted; // a short iterator is padded with black, like embedded-graphics' contract allows
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        FastDraw::fill_rect(self, area, color)
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        let area = Rectangle::new(Point::zero(), self.size());
        FastDraw::fill_rect(self, &area, color)
    }
}

impl<P: WindowPanel> FastDraw for Display<P> {
    type Format = P::Format;

    fn fill_rect(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let Some(c) = clip(area, self.size()) else { return Ok(()) };
        let (px, n) = pixel::encode_one::<P::Format>(color);
        self.panel.begin_write(&c.dst)?;
        self.panel.bus().write_repeat(&px[..n], c.dst.size.width * c.dst.size.height)
    }

    fn blit<S: PixelFormat>(&mut self, area: &Rectangle, src: &[u8], stride_px: usize) -> Result<(), Self::Error> {
        assert!(src.len() >= required_src_len::<S>(area.size, stride_px), "blit source too short");
        let Some(c) = clip(area, self.size()) else { return Ok(()) };
        self.panel.begin_write(&c.dst)?;
        let total = (c.dst.size.width * c.dst.size.height) as usize * P::Format::BYTES;
        let mut stream = BlitStream::<S, P::Format>::new(src, stride_px, &c);
        self.panel.bus().write_with(total, P::Format::BYTES, |buf| stream.fill(buf))
    }
}

// ---------------------------------------------------------------------------
// async
// ---------------------------------------------------------------------------

impl<P: AsyncWindowPanel> AsyncFastDraw for Display<P> {
    type Format = P::Format;
    type Error = AsyncBusError<P>;

    fn size(&self) -> Size {
        self.panel.size()
    }

    async fn fill_rect(
        &mut self,
        area: &Rectangle,
        color: <P::Format as PixelFormat>::Color,
    ) -> Result<(), Self::Error> {
        let Some(c) = clip(area, self.panel.size()) else { return Ok(()) };
        let (px, n) = pixel::encode_one::<P::Format>(color);
        self.panel.begin_write(&c.dst).await?;
        self.panel.bus().write_repeat(&px[..n], c.dst.size.width * c.dst.size.height).await
    }

    async fn blit<S: PixelFormat>(&mut self, area: &Rectangle, src: &[u8], stride_px: usize) -> Result<(), Self::Error> {
        assert!(src.len() >= required_src_len::<S>(area.size, stride_px), "blit source too short");
        let Some(c) = clip(area, self.panel.size()) else { return Ok(()) };
        self.panel.begin_write(&c.dst).await?;
        let total = (c.dst.size.width * c.dst.size.height) as usize * P::Format::BYTES;
        let mut stream = BlitStream::<S, P::Format>::new(src, stride_px, &c);
        self.panel.bus().write_with(total, P::Format::BYTES, |buf| stream.fill(buf)).await
    }
}
