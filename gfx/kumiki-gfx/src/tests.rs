//! Host tests with a recording mock panel.

use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
};

use crate::{
    bus::{blocking::PanelBus, ChunkPlan, ErrorType},
    panel::{blocking::WindowPanel, Panel},
    pixel::{PixelFormat, Rgb565Be, Rgb565Le},
    Display, FastDraw, Sprite,
};

/// Framebuffer-backed mock: interprets begin_write + streamed pixels like a real panel.
struct MockPanel {
    w: u32,
    h: u32,
    fb: Vec<u8>,
    win: Rectangle,
    cursor: usize,
    windows: usize,
    chunk_cap: usize,
}

impl MockPanel {
    fn new(w: u32, h: u32) -> Self {
        Self { w, h, fb: vec![0; (w * h * 2) as usize], win: Rectangle::zero(), cursor: 0, windows: 0, chunk_cap: 7 * 2 }
    }
    fn px(&self, x: u32, y: u32) -> Rgb565 {
        let o = ((y * self.w + x) * 2) as usize;
        Rgb565Be::decode(&self.fb[o..])
    }
    fn put(&mut self, bytes: &[u8]) {
        #[allow(clippy::chunks_exact_to_as_chunks)]
        for px in bytes.chunks_exact(2) {
            let ww = self.win.size.width as usize;
            let (x, y) = (self.cursor % ww, self.cursor / ww);
            let gx = self.win.top_left.x as usize + x;
            let gy = self.win.top_left.y as usize + y;
            assert!(gy < self.win.top_left.y as usize + self.win.size.height as usize, "overrun");
            let o = (gy * self.w as usize + gx) * 2;
            self.fb[o..o + 2].copy_from_slice(px);
            self.cursor += 1;
        }
    }
}

impl ErrorType for MockPanel {
    type Error = core::convert::Infallible;
}

impl PanelBus for MockPanel {
    fn command(&mut self, _cmd: u8, _params: &[u8]) -> Result<(), Self::Error> {
        Ok(())
    }
    fn write_with<F: FnMut(&mut [u8])>(&mut self, total: usize, unit: usize, mut fill: F) -> Result<(), Self::Error> {
        let mut buf = vec![0u8; self.chunk_cap];
        for n in ChunkPlan::new(total, unit, self.chunk_cap) {
            fill(&mut buf[..n]);
            let chunk = buf[..n].to_vec();
            self.put(&chunk);
        }
        Ok(())
    }
    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

impl Panel for MockPanel {
    type Format = Rgb565Be;
    fn size(&self) -> Size {
        Size::new(self.w, self.h)
    }
}

impl WindowPanel for MockPanel {
    type Bus = Self;
    fn begin_write(&mut self, area: &Rectangle) -> Result<(), core::convert::Infallible> {
        assert!(area.top_left.x >= 0 && area.top_left.y >= 0);
        assert!(area.top_left.x as u32 + area.size.width <= self.w);
        assert!(area.top_left.y as u32 + area.size.height <= self.h);
        self.win = *area;
        self.cursor = 0;
        self.windows += 1;
        Ok(())
    }
    fn bus(&mut self) -> &mut Self {
        self
    }
}

#[test]
fn fill_rect_is_clipped() {
    let mut d = Display::new(MockPanel::new(8, 6));
    d.fill_rect(&Rectangle::new(Point::new(-2, 4), Size::new(5, 5)), Rgb565::RED).unwrap();
    let p = d.panel();
    assert_eq!(p.px(0, 4), Rgb565::RED);
    assert_eq!(p.px(2, 5), Rgb565::RED);
    assert_eq!(p.px(3, 5), Rgb565::BLACK);
    assert_eq!(p.px(0, 3), Rgb565::BLACK);
    assert_eq!(p.windows, 1);
}

#[test]
fn blit_converts_and_clips() {
    let mut d = Display::new(MockPanel::new(4, 4));
    // 3x2 source in little-endian RGB565 with stride 5
    let mut src = vec![0u8; 2 * 5 * 2];
    for (i, c) in [Rgb565::RED, Rgb565::GREEN, Rgb565::BLUE].iter().enumerate() {
        Rgb565Le::encode(*c, &mut src[i * 2..]);
        Rgb565Le::encode(*c, &mut src[(5 + i) * 2..]);
    }
    d.blit::<Rgb565Le>(&Rectangle::new(Point::new(2, 3), Size::new(3, 2)), &src, 5).unwrap();
    let p = d.panel();
    assert_eq!(p.px(2, 3), Rgb565::RED);
    assert_eq!(p.px(3, 3), Rgb565::GREEN);
    assert_eq!(p.px(1, 3), Rgb565::BLACK);
}

#[test]
fn draw_iter_batches_runs() {
    let mut d = Display::new(MockPanel::new(16, 4));
    Rectangle::new(Point::new(1, 1), Size::new(10, 2))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::YELLOW))
        .draw(&mut d)
        .unwrap();
    assert_eq!(d.panel().px(10, 2), Rgb565::YELLOW);
    // A styled rectangle goes through fill_solid → one window.
    assert_eq!(d.panel().windows, 1);

    let before = d.panel().windows;
    let line: Vec<_> = (0..12).map(|x| Pixel(Point::new(x, 0), Rgb565::CYAN)).collect();
    d.draw_iter(line).unwrap();
    assert_eq!(d.panel().windows - before, 1, "a horizontal run is one window");
}

#[test]
fn fill_contiguous_clipped() {
    let mut d = Display::new(MockPanel::new(4, 4));
    let area = Rectangle::new(Point::new(-1, -1), Size::new(3, 3));
    let colors = (0..9).map(|i| if i == 4 { Rgb565::WHITE } else { Rgb565::BLUE });
    d.fill_contiguous(&area, colors).unwrap();
    assert_eq!(d.panel().px(0, 0), Rgb565::WHITE);
    assert_eq!(d.panel().px(1, 1), Rgb565::BLUE);
    assert_eq!(d.panel().px(2, 2), Rgb565::BLACK);
}

#[test]
fn sprite_keyed_push() {
    let mut s = Sprite::<Rgb565Be, _>::new([0u8; 4 * 2 * 2], 4, 2).unwrap();
    s.clear(Rgb565::MAGENTA).unwrap();
    s.fill_rect(&Rectangle::new(Point::new(1, 0), Size::new(2, 2)), Rgb565::GREEN).unwrap();
    let mut d = Display::new(MockPanel::new(6, 3));
    d.clear(Rgb565::WHITE).unwrap();
    s.push_keyed(&mut d, Point::new(1, 1), Rgb565::MAGENTA).unwrap();
    let p = d.panel();
    assert_eq!(p.px(1, 1), Rgb565::WHITE); // keyed out
    assert_eq!(p.px(2, 1), Rgb565::GREEN);
    assert_eq!(p.px(3, 2), Rgb565::GREEN);
    assert_eq!(p.px(4, 2), Rgb565::WHITE);
}

#[test]
fn sprite_draws_embedded_graphics_and_pushes() {
    let mut s = Sprite::<Rgb565Be, _>::new(vec![0u8; 10 * 10 * 2], 10, 10).unwrap();
    embedded_graphics::primitives::Circle::new(Point::new(0, 0), 10)
        .into_styled(PrimitiveStyle::with_fill(Rgb565::RED))
        .draw(&mut s)
        .unwrap();
    let mut d = Display::new(MockPanel::new(10, 10));
    s.push(&mut d, Point::zero()).unwrap();
    assert_eq!(d.panel().px(5, 5), Rgb565::RED);
    assert_eq!(d.panel().px(0, 0), Rgb565::BLACK);
}

#[test]
fn buffered_flush_aligns_dirty_rect() {
    use crate::BufferedDisplay;
    let d = Display::new(MockPanel::new(8, 4));
    let mut b = BufferedDisplay::<_, Rgb565Be, _>::new(d, Size::new(8, 4), vec![0u8; 8 * 4 * 2], 2).unwrap();
    b.flush().unwrap(); // initial full flush
    let before = b.target_mut().panel().windows;
    // One odd pixel at x=3 → flushed window must be x=2..3 (aligned to 2).
    Pixel(Point::new(3, 1), Rgb565::GREEN).draw(&mut b).unwrap();
    b.flush().unwrap();
    let p = b.target_mut().panel();
    assert_eq!(p.windows - before, 1);
    assert_eq!(p.win, Rectangle::new(Point::new(2, 1), Size::new(2, 1)));
    assert_eq!(p.px(3, 1), Rgb565::GREEN);
    // Nothing dirty → no transfer.
    let n = p.windows;
    b.flush().unwrap();
    assert_eq!(b.target_mut().panel().windows, n);
}
