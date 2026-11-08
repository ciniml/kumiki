//! kumiki demo: the same binary runs on M5Stack CoreS3 and AtomS3.
//!
//! Button A (AtomS3: the screen button, CoreS3: touch anywhere) cycles pages:
//! 0. board info + colour bars
//! 1. IMU readings (or "no IMU")
//! 2. keyed-sprite animation
#![no_std]
#![no_main]

use core::fmt::Write as _;

use embedded_graphics::{
    mono_font::{MonoFont, MonoTextStyleBuilder, ascii::FONT_6X10, ascii::FONT_10X20},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, PrimitiveStyle, Rectangle},
    text::Text,
};
use esp_backtrace as _;
use esp_hal::{clock::CpuClock, delay::Delay, main, time::Instant};
use esp_println::println;
use kumiki_boards::{AnyDisplay, Devices};
use kumiki_class::{Backlight, ButtonKind, Buttons, Imu, Power, Touch, TouchPoint};
use kumiki_gfx::{FastDraw, Rgb565Be, Sprite};

esp_bootloader_esp_idf::esp_app_desc!();

/// Small fixed-capacity string for formatting without an allocator.
struct Line<const N: usize> {
    buf: [u8; N],
    len: usize,
}

impl<const N: usize> Line<N> {
    fn new() -> Self {
        Self { buf: [0; N], len: 0 }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..self.len]).unwrap_or("")
    }
}

impl<const N: usize> core::fmt::Write for Line<N> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let n = s.len().min(N - self.len);
        self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
        self.len += n;
        Ok(())
    }
}

fn font_for(d: &AnyDisplay) -> &'static MonoFont<'static> {
    if d.size().width >= 240 { &FONT_10X20 } else { &FONT_6X10 }
}

fn text(d: &mut AnyDisplay, s: &str, row: i32, color: Rgb565) {
    let font = font_for(d);
    let style = MonoTextStyleBuilder::new().font(font).text_color(color).background_color(Rgb565::BLACK).build();
    let h = font.character_size.height as i32;
    // Clear the rest of the line so shorter text does not leave stale glyphs.
    let w = d.size().width;
    d.fill_rect(&Rectangle::new(Point::new(0, row * h + 2), Size::new(w, h as u32)), Rgb565::BLACK).ok();
    Text::new(s, Point::new(2, row * h + 2 + font.baseline as i32), style).draw(d).ok();
}

fn draw_page0(d: &mut AnyDisplay, name: &str) {
    d.clear(Rgb565::BLACK).ok();
    let size = d.size();
    let bars = [Rgb565::RED, Rgb565::GREEN, Rgb565::BLUE, Rgb565::YELLOW, Rgb565::CYAN, Rgb565::MAGENTA, Rgb565::WHITE];
    let bw = size.width / bars.len() as u32;
    let top = size.height as i32 / 2;
    for (i, c) in bars.iter().enumerate() {
        d.fill_rect(&Rectangle::new(Point::new(i as i32 * bw as i32, top), Size::new(bw, size.height / 2)), *c).ok();
    }
    text(d, "kumiki", 0, Rgb565::new(31, 40, 0));
    text(d, name, 1, Rgb565::WHITE);
    text(d, "A: next page", 2, Rgb565::new(16, 32, 16));
}

#[main]
fn main() -> ! {
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let delay = Delay::new();

    let dev = match kumiki_boards::init(p) {
        Ok(d) => d,
        Err(e) => panic!("board init failed: {e:?}"),
    };
    let Devices { board, mut display, mut backlight, mut touch, mut imu, mut power, mut buttons, .. } = dev;
    println!(
        "kumiki-demo: detected {} (touch: {}, imu: {}, power: {})",
        board.name(),
        touch.as_ref().map(|t| t.name()).unwrap_or("none"),
        imu.is_some(),
        power.is_some()
    );
    backlight.set_brightness(200).ok();

    let mut sprite_buf = [0u8; 32 * 32 * 2];
    let mut spr = Sprite::<Rgb565Be, _>::new(&mut sprite_buf[..], 32, 32).unwrap();
    spr.clear(Rgb565::MAGENTA).ok();
    Circle::new(Point::new(1, 1), 30).into_styled(PrimitiveStyle::with_fill(Rgb565::YELLOW)).draw(&mut spr).ok();
    Circle::new(Point::new(9, 9), 6).into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK)).draw(&mut spr).ok();
    Circle::new(Point::new(18, 9), 6).into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK)).draw(&mut spr).ok();

    let mut btn = Buttons::default();
    let mut page = 0u8;
    let mut redraw = true;
    let mut frame = 0i32;
    let (mut touch_ok, mut touch_err, mut touch_max) = (0u32, 0u32, 0usize);
    let (mut x, mut y, mut vx, mut vy) = (0i32, 0i32, 3i32, 2i32);

    loop {
        let now = Instant::now().duration_since_epoch().as_millis();
        let raw = buttons.raw(touch.as_mut());
        btn.update(raw, now, |e| {
            if e.kind == ButtonKind::Click {
                page = (page + 1) % 3;
                redraw = true;
                println!("button {:?} click -> page {}", e.id, page);
            }
        });

        let size = display.size();
        match page {
            0 => {
                if redraw {
                    draw_page0(&mut display, board.name());
                }
                if frame % 30 == 0 {
                    if let Some(pw) = power.as_mut() {
                        let mut l = Line::<32>::new();
                        let mv = pw.battery_mv().ok().flatten().unwrap_or(0);
                        let usb = pw.vbus_present().ok().flatten().unwrap_or(false);
                        let _ = write!(l, "bat {}mV usb {}", mv, if usb { "yes" } else { "no" });
                        text(&mut display, l.as_str(), 3, Rgb565::new(20, 40, 31));
                    }
                }
            }
            1 => {
                if redraw {
                    display.clear(Rgb565::BLACK).ok();
                    text(&mut display, "IMU", 0, Rgb565::new(31, 40, 0));
                }
                match imu.as_mut() {
                    Some(m) => {
                        if let (Ok(a), Ok(g)) = (m.accel(), m.gyro()) {
                            let mut l = Line::<32>::new();
                            let _ = write!(l, "ax {:+.2}", a.x);
                            text(&mut display, l.as_str(), 1, Rgb565::WHITE);
                            let mut l = Line::<32>::new();
                            let _ = write!(l, "ay {:+.2}", a.y);
                            text(&mut display, l.as_str(), 2, Rgb565::WHITE);
                            let mut l = Line::<32>::new();
                            let _ = write!(l, "az {:+.2}", a.z);
                            text(&mut display, l.as_str(), 3, Rgb565::WHITE);
                            let mut l = Line::<32>::new();
                            let _ = write!(l, "gz {:+.0}", g.z);
                            text(&mut display, l.as_str(), 4, Rgb565::new(16, 63, 16));
                        }
                    }
                    None => {
                        if redraw {
                            text(&mut display, "no IMU driver", 1, Rgb565::new(31, 20, 0));
                        }
                    }
                }
            }
            _ => {
                if redraw {
                    display.clear(Rgb565::new(0, 8, 12)).ok();
                }
                // Erase the old position, move, draw keyed sprite.
                display.fill_rect(&Rectangle::new(Point::new(x, y), Size::new(32, 32)), Rgb565::new(0, 8, 12)).ok();
                x += vx;
                y += vy;
                if x < 0 || x + 32 > size.width as i32 {
                    vx = -vx;
                    x += 2 * vx;
                }
                if y < 0 || y + 32 > size.height as i32 {
                    vy = -vy;
                    y += 2 * vy;
                }
                spr.push_keyed(&mut display, Point::new(x, y), Rgb565::MAGENTA).ok();
            }
        }
        // Touch: draw a dot where the panel is touched (every board with touch).
        if let Some(t) = touch.as_mut() {
            let mut pts = [TouchPoint::default(); 2];
            let r = t.read(&mut pts);
            match &r {
                Ok(n) => {
                    touch_ok += 1;
                    touch_max = touch_max.max(*n);
                }
                Err(_) => touch_err += 1,
            }
            if frame % 120 == 0 {
                println!("touch diag: ok {touch_ok}, err {touch_err}, max points {touch_max}");
                (touch_ok, touch_err, touch_max) = (0, 0, 0);
            }
            if let Ok(n) = r {
                for p in &pts[..n] {
                    Circle::with_center(Point::new(p.x as i32, p.y as i32), 9)
                        .into_styled(PrimitiveStyle::with_fill(Rgb565::RED))
                        .draw(&mut display)
                        .ok();
                }
                if n > 0 && frame % 10 == 0 {
                    println!("touch: {} point(s), first at ({}, {})", n, pts[0].x, pts[0].y);
                }
            }
        }

        display.flush().ok(); // frame-buffered displays (StopWatch) send changes now
        redraw = false;
        frame += 1;
        delay.delay_millis(16);
    }
}
