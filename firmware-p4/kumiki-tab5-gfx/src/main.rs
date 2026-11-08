//! kumiki-gfx on the Tab5: [`FbDisplay`] over the DSI panel
//! ([`kumiki_p4v1_hal::fb::DsiPanel`], double-buffered, PPA fills).
//!
//! Draws a screen with embedded-graphics (text, shapes), then animates a
//! bouncing box with incremental redraws, printing the presents per second
//! and how long a full-screen clear (PPA) and a full present take.

#![no_std]
#![no_main]

#[path = "../../kumiki-tab5-demo/src/tab5.rs"]
mod tab5;

use embedded_graphics::{
    mono_font::{MonoTextStyle, ascii::FONT_10X20},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, PrimitiveStyle, Rectangle, Triangle},
    text::Text,
};
use kumiki_gfx::{FastDraw, FbDisplay};
use kumiki_p4v1_hal::{
    self as hal, cache, clock,
    dsi::{self, Dsi, FB_BYTES, HEIGHT, WIDTH},
    fb::DsiPanel,
    gpio::Output,
    i2c::I2c,
    println, psram, time,
};

hal::entry!(main);

fn us_since(t: u64) -> u64 {
    (time::now() - t) / time::TICKS_PER_US
}

const BG: Rgb565 = Rgb565::new(2, 6, 8);

/// The static part of the screen.
fn draw_scene(d: &mut FbDisplay<DsiPanel>) {
    let _ = d.clear(BG);
    let text = MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE);
    let _ = Text::new("kumiki-gfx on M5Stack Tab5", Point::new(40, 60), text).draw(d);
    let _ = Text::new("ESP32-P4 rev v1 / kumiki-p4v1-hal / FbDisplay", Point::new(40, 90), text).draw(d);
    let bars = [Rgb565::RED, Rgb565::GREEN, Rgb565::BLUE, Rgb565::CYAN, Rgb565::MAGENTA, Rgb565::YELLOW, Rgb565::WHITE];
    for (i, c) in bars.iter().enumerate() {
        let _ = d.fill_rect(&Rectangle::new(Point::new(40 + i as i32 * 90, 120), Size::new(80, 80)), *c);
    }
    let _ = Circle::new(Point::new(60, 240), 160).into_styled(PrimitiveStyle::with_stroke(Rgb565::YELLOW, 6)).draw(d);
    let _ = Triangle::new(Point::new(300, 400), Point::new(460, 240), Point::new(620, 400))
        .into_styled(PrimitiveStyle::with_fill(Rgb565::CSS_ORANGE))
        .draw(d);
}

fn main() -> ! {
    println!();
    println!("kumiki-tab5-gfx");
    if dsi::stop() {
        println!("display DMA was running; stopped");
    }
    cache::set_l2_line();
    let (base, size) = psram::init().unwrap_or_else(|e| halt(e));
    match clock::init_max() {
        Ok(mhz) => println!("clock: {mhz} MHz"),
        Err(e) => println!("clock: {e}"),
    }
    let p = hal::take().unwrap();
    let mut i2c = I2c::new(p.i2c1, p.pins.gpio31, p.pins.gpio32, 100_000);
    let panel = match tab5::init(&mut i2c, p.pins.gpio23.degrade()) {
        tab5::Panel::Ili9881c { .. } => dsi::Panel::Ili9881c,
        tab5::Panel::St7123 => dsi::Panel::St7123,
        _ => halt("no supported panel"),
    };
    let top = base + size - (6 << 20);
    let fbs = [top, top + FB_BYTES, top + 2 * FB_BYTES];
    cache::autoload(fbs[0] - base);
    // All three start out cleared.
    for fb in fbs {
        unsafe { core::ptr::write_bytes(fb as *mut u8, 0, FB_BYTES) };
        dsi::flush(fb, FB_BYTES);
    }
    let dsi = Dsi::init(panel, fbs[0]).unwrap_or_else(|e| halt(e));
    let _backlight = Output::new(p.pins.gpio22, true);

    let mut d = FbDisplay::new(DsiPanel::new(dsi, fbs));
    let t = time::now();
    let _ = d.clear(BG);
    println!("clear (PPA) {} us", us_since(t));
    let t = time::now();
    draw_scene(&mut d);
    println!("scene draw {} us", us_since(t));
    let t = time::now();
    let r = d.flush();
    println!("first present (full frame) {r:?} in {} us", us_since(t));

    // A box bouncing in the lower part; erase = fill with the background.
    const BOX: u32 = 120;
    let area = Rectangle::new(Point::new(0, 460), Size::new(WIDTH as u32, HEIGHT as u32 - 460));
    let (mut pos, mut vel) = (Point::new(40, 500), Point::new(9, 7));
    let colors = [Rgb565::CSS_ORANGE, Rgb565::CSS_PURPLE, Rgb565::CSS_TEAL];
    let mut prev: Option<Rectangle> = None;
    let (mut presents, mut last) = (0u32, time::now());
    let mut worst_us = 0u64;
    loop {
        pos += vel;
        let br = area.bottom_right().unwrap() - Point::new(BOX as i32, BOX as i32);
        if pos.x < area.top_left.x || pos.x > br.x {
            vel.x = -vel.x;
            pos.x = pos.x.clamp(area.top_left.x, br.x);
        }
        if pos.y < area.top_left.y || pos.y > br.y {
            vel.y = -vel.y;
            pos.y = pos.y.clamp(area.top_left.y, br.y);
        }
        if let Some(r) = prev {
            let _ = d.fill_rect(&r, BG);
        }
        let r = Rectangle::new(pos, Size::new(BOX, BOX));
        let c = colors[(presents as usize / 60) % colors.len()];
        let _ = r.into_styled(PrimitiveStyle::with_fill(c)).draw(&mut d);
        let _ = Text::new("kumiki", pos + Point::new(30, 66), MonoTextStyle::new(&FONT_10X20, Rgb565::WHITE)).draw(&mut d);
        prev = Some(r);
        let t = time::now();
        if let Err(e) = d.flush() {
            println!("present failed: {e:?}");
        }
        worst_us = worst_us.max(us_since(t));
        presents += 1;
        if time::now() - last >= 1_000_000 * time::TICKS_PER_US {
            let (total, waited) = d.panel_mut().stats();
            let dsi = d.panel_mut().dsi_mut();
            println!(
                "presents/s {}, worst present {} us, waited {waited}/{total}, frames {}, underruns {}, restarts {}",
                presents,
                worst_us,
                dsi.frames(),
                dsi.underruns(),
                dsi.restarts()
            );
            presents = 0;
            worst_us = 0;
            last = time::now();
        }
    }
}

fn halt(why: &str) -> ! {
    println!("*** halted: {why}");
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("\n*** panic: {info}");
    loop {
        core::hint::spin_loop();
    }
}
