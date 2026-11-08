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
fn viewport_translates_and_clips() {
    use crate::Viewport;
    let mut d = Display::new(MockPanel::new(10, 10));
    {
        let mut v = Viewport::new(&mut d, Rectangle::new(Point::new(3, 2), Size::new(4, 4)));
        assert_eq!(v.size(), Size::new(4, 4));
        // Overhanging fill is clipped to the viewport.
        v.fill_rect(&Rectangle::new(Point::new(-1, -1), Size::new(3, 3)), Rgb565::RED).unwrap();
        Pixel(Point::new(3, 3), Rgb565::GREEN).draw(&mut v).unwrap();
        Pixel(Point::new(4, 0), Rgb565::BLUE).draw(&mut v).unwrap(); // outside → dropped
        // Blit of a 3×3 block at (2,2): only its top-left 2×2 lands inside.
        let mut src = vec![0u8; 3 * 3 * 2];
        #[allow(clippy::chunks_exact_to_as_chunks)]
        for (i, px) in src.chunks_exact_mut(2).enumerate() {
            Rgb565Be::encode(if i == 0 { Rgb565::WHITE } else { Rgb565::CYAN }, px);
        }
        v.blit::<Rgb565Be>(&Rectangle::new(Point::new(2, 2), Size::new(3, 3)), &src, 3).unwrap();
    }
    let p = d.panel();
    assert_eq!(p.px(3, 2), Rgb565::RED);
    assert_eq!(p.px(4, 3), Rgb565::RED);
    assert_eq!(p.px(2, 2), Rgb565::BLACK);
    assert_eq!(p.px(7, 2), Rgb565::BLACK); // the dropped pixel would be at x = 7
    assert_eq!(p.px(5, 4), Rgb565::WHITE);
    assert_eq!(p.px(6, 5), Rgb565::CYAN);
    assert_eq!(p.px(7, 6), Rgb565::BLACK);
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

/// Scan-out mock: records cache flushes / presents, optional "2D engine".
struct MockFb {
    w: u32,
    h: u32,
    fb: Vec<u8>,
    flushed: Vec<Rectangle>,
    presents: Vec<Option<Rectangle>>,
    hw: bool,
    hw_fills: usize,
    hw_copies: usize,
}

impl MockFb {
    fn new(w: u32, h: u32, hw: bool) -> Self {
        Self { w, h, fb: vec![0; (w * h * 2) as usize], flushed: vec![], presents: vec![], hw, hw_fills: 0, hw_copies: 0 }
    }
    fn px(&self, x: u32, y: u32) -> Rgb565 {
        Rgb565Le::decode(&self.fb[((y * self.w + x) * 2) as usize..])
    }
}

impl crate::FramebufferPanel for MockFb {
    type Format = Rgb565Le;
    type Error = ();
    fn size(&self) -> Size {
        Size::new(self.w, self.h)
    }
    fn back_buffer(&mut self) -> &mut [u8] {
        &mut self.fb
    }
    fn flush_cache(&mut self, area: &Rectangle) {
        self.flushed.push(*area);
    }
    fn present(&mut self, changed: Option<&Rectangle>) -> Result<(), ()> {
        self.presents.push(changed.copied());
        Ok(())
    }
    fn hw_fill(&mut self, area: &Rectangle, pixel: &[u8]) -> bool {
        if !self.hw {
            return false;
        }
        for p in area.points() {
            let o = ((p.y as u32 * self.w + p.x as u32) * 2) as usize;
            self.fb[o..o + 2].copy_from_slice(pixel);
        }
        self.hw_fills += 1;
        true
    }
    fn hw_copy(&mut self, _src: &Rectangle, _dst: Point) -> bool {
        self.hw_copies += 1;
        false // fall back to the CPU path
    }
    fn hw_threshold(&self) -> u32 {
        16
    }
}

#[test]
fn fb_display_tracks_dirty_and_presents() {
    let mut d = crate::FbDisplay::new(MockFb::new(20, 10, false));
    d.flush().unwrap(); // initial: whole frame
    Pixel(Point::new(3, 4), Rgb565::RED).draw(&mut d).unwrap();
    Rectangle::new(Point::new(10, 2), Size::new(2, 2)).into_styled(PrimitiveStyle::with_fill(Rgb565::GREEN)).draw(&mut d).unwrap();
    assert_eq!(d.dirty(), Some(Rectangle::new(Point::new(3, 2), Size::new(9, 3))));
    d.flush().unwrap();
    d.flush().unwrap(); // nothing changed
    let p = d.into_panel();
    assert_eq!(p.px(3, 4), Rgb565::RED);
    assert_eq!(p.px(11, 3), Rgb565::GREEN);
    assert_eq!(p.flushed, vec![Rectangle::new(Point::zero(), Size::new(20, 10)), Rectangle::new(Point::new(3, 2), Size::new(9, 3))]);
    assert_eq!(p.presents.len(), 3);
    assert_eq!(p.presents[2], None);
}

#[test]
fn fb_display_uses_hw_fill_above_threshold() {
    let mut d = crate::FbDisplay::new(MockFb::new(20, 10, true));
    d.fill_rect(&Rectangle::new(Point::new(0, 0), Size::new(3, 3)), Rgb565::BLUE).unwrap(); // 9 px: CPU
    d.fill_rect(&Rectangle::new(Point::new(5, 5), Size::new(30, 30)), Rgb565::RED).unwrap(); // clipped 15x5: hw
    let p = d.into_panel();
    assert_eq!(p.hw_fills, 1);
    assert_eq!(p.px(2, 2), Rgb565::BLUE);
    assert_eq!(p.px(19, 9), Rgb565::RED);
    assert_eq!(p.px(4, 9), Rgb565::BLACK);
}

#[test]
fn sin_cos_accuracy() {
    for d in (-720..=720).step_by(7) {
        let (s, c) = crate::sprite::sin_cos_deg(d as f32);
        let r = (d as f64).to_radians();
        assert!((s as f64 - r.sin()).abs() < 2e-6, "sin {d}");
        assert!((c as f64 - r.cos()).abs() < 2e-6, "cos {d}");
    }
}

#[test]
fn sprite_rotate_zoom() {
    // 4×2 source: row 0 = R G B Y, row 1 = magenta (key) everywhere but (0,1) = white.
    let mut sbuf = [0u8; 4 * 2 * 2];
    let mut spr = Sprite::<Rgb565Be, _>::new(&mut sbuf[..], 4, 2).unwrap();
    for (x, c) in [Rgb565::RED, Rgb565::GREEN, Rgb565::BLUE, Rgb565::YELLOW].iter().enumerate() {
        Pixel(Point::new(x as i32, 0), *c).draw(&mut spr).unwrap();
    }
    for x in 0..4 {
        Pixel(Point::new(x, 1), if x == 0 { Rgb565::WHITE } else { Rgb565::MAGENTA }).draw(&mut spr).unwrap();
    }
    let px = |t: &Sprite<Rgb565Be, &mut [u8]>, x: i32, y: i32| Rgb565Be::decode(&t.bytes()[((y * 10 + x) * 2) as usize..]);

    // 90° clockwise about (5, 5): source x runs down the screen, source y right-to-left.
    let mut tbuf = [0u8; 10 * 10 * 2];
    let mut t = Sprite::<Rgb565Be, _>::new(&mut tbuf[..], 10, 10).unwrap();
    spr.push_rotate_zoom(&mut t, Point::new(5, 5), 90.0, 1.0, 1.0, Some(Rgb565::MAGENTA)).unwrap();
    assert_eq!(px(&t, 5, 3), Rgb565::RED);
    assert_eq!(px(&t, 5, 4), Rgb565::GREEN);
    assert_eq!(px(&t, 5, 5), Rgb565::BLUE);
    assert_eq!(px(&t, 5, 6), Rgb565::YELLOW);
    assert_eq!(px(&t, 4, 3), Rgb565::WHITE);
    assert_eq!(px(&t, 4, 4), Rgb565::BLACK, "keyed pixel left the target untouched");

    // Unrotated, zoom 2: every source pixel becomes 2×2.
    let mut tbuf = [0u8; 10 * 10 * 2];
    let mut t = Sprite::<Rgb565Be, _>::new(&mut tbuf[..], 10, 10).unwrap();
    spr.push_rotate_zoom(&mut t, Point::new(5, 5), 0.0, 2.0, 2.0, None).unwrap();
    // Source (0,0) covers destination x 1..3, y 3..5.
    for (x, y) in [(1, 3), (2, 3), (1, 4), (2, 4)] {
        assert_eq!(px(&t, x, y), Rgb565::RED, "({x},{y})");
    }
    assert_eq!(px(&t, 7, 3), Rgb565::YELLOW);
    assert_eq!(px(&t, 3, 5), Rgb565::MAGENTA, "no key: magenta drawn");
    assert_eq!(px(&t, 0, 3), Rgb565::BLACK);
}

#[test]
fn rotation_maps_round_trip() {
    use crate::Rotation;
    let native = Size::new(4, 6);
    for r in [Rotation::Deg0, Rotation::Deg90, Rotation::Deg180, Rotation::Deg270] {
        let l = r.logical_size(native);
        for y in 0..l.height as i32 {
            for x in 0..l.width as i32 {
                let p = Point::new(x, y);
                let n = r.to_native(p, native);
                assert!(n.x >= 0 && n.y >= 0 && n.x < 4 && n.y < 6, "{r:?} {p:?} -> {n:?}");
                assert_eq!(r.to_logical(n, native), p, "{r:?}");
            }
        }
    }
    // 90° clockwise: logical top-left is native top-right.
    assert_eq!(Rotation::Deg90.to_native(Point::zero(), native), Point::new(3, 0));
    // 270°: logical top-left is native bottom-left.
    assert_eq!(Rotation::Deg270.to_native(Point::zero(), native), Point::new(0, 5));
}

#[test]
fn fb_display_rotated_drawing() {
    use crate::Rotation;
    for r in [Rotation::Deg90, Rotation::Deg180, Rotation::Deg270] {
        let mut d = crate::FbDisplay::new(MockFb::new(6, 4, true)).with_rotation(r);
        let ls = d.size();
        assert_eq!(ls, r.logical_size(Size::new(6, 4)));
        d.flush().unwrap();
        // A pixel, a filled rect (hw path above 16 px) and a blit, all logical.
        Pixel(Point::new(1, 0), Rgb565::RED).draw(&mut d).unwrap();
        d.fill_rect(&Rectangle::new(Point::new(0, 2), Size::new(ls.width, 2)), Rgb565::GREEN).unwrap();
        let mut src = [0u8; 4];
        Rgb565Be::encode(Rgb565::BLUE, &mut src[0..2]);
        Rgb565Be::encode(Rgb565::YELLOW, &mut src[2..4]);
        d.blit::<Rgb565Be>(&Rectangle::new(Point::new(2, 1), Size::new(2, 1)), &src, 2).unwrap();
        let dirty = d.dirty().unwrap();
        let p = d.into_panel();
        let at = |l: Point| {
            let n = r.to_native(l, Size::new(6, 4));
            assert!(dirty.contains(n), "{r:?}: {n:?} outside dirty {dirty:?}");
            p.px(n.x as u32, n.y as u32)
        };
        assert_eq!(at(Point::new(1, 0)), Rgb565::RED, "{r:?}");
        assert_eq!(at(Point::new(0, 3)), Rgb565::GREEN, "{r:?}");
        assert_eq!(at(Point::new(2, 1)), Rgb565::BLUE, "{r:?}");
        assert_eq!(at(Point::new(3, 1)), Rgb565::YELLOW, "{r:?}");
        assert_eq!(at(Point::new(0, 0)), Rgb565::BLACK, "{r:?}");
    }
}

#[test]
fn fb_display_copy_within_overlapping() {
    let mut d = crate::FbDisplay::new(MockFb::new(8, 8, false));
    for y in 0..8 {
        Pixel(Point::new(1, y), Rgb565::new(y as u8, 0, 0)).draw(&mut d).unwrap();
    }
    // Scroll down by 2 (overlapping rows): must copy bottom-up.
    d.copy_within(&Rectangle::new(Point::new(0, 0), Size::new(8, 6)), Point::new(0, 2));
    let p = d.into_panel();
    for y in 2..8 {
        assert_eq!(p.px(1, y), Rgb565::new(y as u8 - 2, 0, 0));
    }
    assert_eq!(p.hw_copies, 1);
}
