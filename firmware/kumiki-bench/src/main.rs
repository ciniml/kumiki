//! kumiki-gfx benchmark firmware (ESP32-S3: M5Stack CoreS3 / AtomS3).
//!
//! Scenarios follow doc/10-performance-plan.md (B1, B2, B3, B5, B8). Results are
//! printed as CSV lines:
//! `bench,<board>,kumiki,<id>,<frames>,<us_per_frame>,<fps_x100>`
//!
//! Board selection is a Cargo feature for now (`cores3` / `atoms3`); runtime
//! detection arrives with kumiki-boards (milestone M2).
#![no_std]
#![no_main]

#[cfg(all(feature = "cores3", feature = "atoms3"))]
compile_error!("enable only one of `cores3` / `atoms3`");
#[cfg(not(any(feature = "cores3", feature = "atoms3")))]
compile_error!("enable one of `cores3` / `atoms3`");

use embedded_graphics::{
    mono_font::{MonoTextStyle, ascii::FONT_6X10},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, PrimitiveStyle, Rectangle},
    text::Text,
};
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
    dma_tx_buffer,
    gpio::{Level, Output, OutputConfig},
    main,
    spi::{
        Mode,
        master::{Config as SpiConfig, Spi},
    },
    time::{Instant, Rate},
};
use esp_println::println;
use kumiki_gfx::{Display, FastDraw, Rgb565Be, Rgb888Fmt, Sprite};
use kumiki_gfx_bus_esp::EspSpiDmaBus;
use kumiki_gfx_panels::{DcsConfig, DcsPanel, NoPin};

mod board;

esp_bootloader_esp_idf::esp_app_desc!();

const W: usize = board::WIDTH as usize;
const H: usize = board::HEIGHT as usize;

/// Full-screen frame in wire order, in internal RAM (`.bss`).
static mut FRAME: [u8; W * H * 2] = [0; W * H * 2];
/// RGB888 band (W × BAND_H) for the conversion benchmark.
const BAND_H: usize = if H >= 40 { 40 } else { H };
static mut BAND888: [u8; W * BAND_H * 3] = [0; W * BAND_H * 3];

fn report(id: &str, frames: u32, start: Instant) {
    let us = start.elapsed().as_micros() as u32;
    let per = us / frames;
    let fps_x100 = if per > 0 { 100_000_000 / per } else { 0 };
    println!("bench,{},kumiki,{},{},{},{}.{:02}", board::NAME, id, frames, per, fps_x100 / 100, fps_x100 % 100);
}

#[main]
fn main() -> ! {
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let mut delay = Delay::new();
    println!("kumiki-bench on {}", board::NAME);

    let pins = board::power_on(&mut delay, board::Parts::take(p));

    let spi = Spi::new(pins.spi, SpiConfig::default().with_frequency(Rate::from_mhz(40)).with_mode(Mode::_0))
        .unwrap()
        .with_sck(pins.sck)
        .with_mosi(pins.mosi)
        .with_dma(pins.dma);
    let dc = Output::new(pins.dc, Level::High, OutputConfig::default());
    let cs = Output::new(pins.cs, Level::High, OutputConfig::default());
    let bus = EspSpiDmaBus::new(spi, dc, Some(cs), dma_tx_buffer!(16384).unwrap(), dma_tx_buffer!(16384).unwrap());

    let cfg = DcsConfig { ..board::panel_config() };
    let mut panel = DcsPanel::new(bus, board::Model, cfg, None::<NoPin>);
    panel.init(&mut delay).unwrap();
    let mut d = Display::new(panel);
    d.clear(Rgb565::BLACK).unwrap();
    board::backlight_on(&mut delay);

    let full = Rectangle::new(Point::zero(), Size::new(W as u32, H as u32));
    let frames = 60u32;

    // B1: full-screen solid fill.
    let colors = [Rgb565::RED, Rgb565::GREEN, Rgb565::BLUE, Rgb565::WHITE];
    let t = Instant::now();
    for i in 0..frames {
        d.fill_rect(&full, colors[i as usize % 4]).unwrap();
    }
    report("B1_fill", frames, t);

    // B2: full-screen blit from internal RAM, same format (copy into DMA buffers, pipelined).
    let frame: &mut [u8] = unsafe { &mut *core::ptr::addr_of_mut!(FRAME) };
    {
        let mut s = Sprite::<Rgb565Be, _>::new(&mut *frame, W as u16, H as u16).unwrap();
        for y in 0..H as i32 {
            let c = Rgb565::new((y * 31 / H as i32) as u8, 0, 31 - (y * 31 / H as i32) as u8);
            s.fill_rect(&Rectangle::new(Point::new(0, y), Size::new(W as u32, 1)), c).unwrap();
        }
    }
    let t = Instant::now();
    for _ in 0..frames {
        d.blit::<Rgb565Be>(&full, frame, W).unwrap();
    }
    report("B2_blit565", frames, t);

    // B3: RGB888 band → RGB565 conversion fused into the transfer.
    let band: &mut [u8] = unsafe { &mut *core::ptr::addr_of_mut!(BAND888) };
    for (i, px) in band.chunks_exact_mut(3).enumerate() {
        let x = (i % W) as u32;
        px.copy_from_slice(&[(x * 255 / W as u32) as u8, 128, 255 - (x * 255 / W as u32) as u8]);
    }
    let bands = H.div_ceil(BAND_H);
    let t = Instant::now();
    for _ in 0..frames {
        for b in 0..bands {
            let r = Rectangle::new(Point::new(0, (b * BAND_H) as i32), Size::new(W as u32, BAND_H as u32));
            d.blit::<Rgb888Fmt>(&r, band, W).unwrap();
        }
    }
    report("B3_blit888", frames, t);

    // B5: 64 keyed 32×32 sprites per frame (over the B2 background).
    let mut spr_buf = [0u8; 32 * 32 * 2];
    let mut spr = Sprite::<Rgb565Be, _>::new(&mut spr_buf[..], 32, 32).unwrap();
    spr.clear(Rgb565::MAGENTA).unwrap();
    Circle::new(Point::new(1, 1), 30).into_styled(PrimitiveStyle::with_fill(Rgb565::YELLOW)).draw(&mut spr).unwrap();
    let t = Instant::now();
    for f in 0..frames as i32 {
        for k in 0..64i32 {
            let x = (k * 37 + f * 3) % (W as i32 - 16) - 8;
            let y = (k * 23 + f * 2) % (H as i32 - 16) - 8;
            spr.push_keyed(&mut d, Point::new(x, y), Rgb565::MAGENTA).unwrap();
        }
    }
    report("B5_sprite_keyed", frames, t);

    // B8: embedded-graphics primitives and text straight to the display.
    let style = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);
    let t = Instant::now();
    for f in 0..frames as i32 {
        for k in 0..20i32 {
            Circle::new(Point::new((k * 13 + f) % W as i32, (k * 7) % H as i32), 20)
                .into_styled(PrimitiveStyle::with_stroke(Rgb565::CYAN, 1))
                .draw(&mut d)
                .unwrap();
        }
        Text::new("kumiki", Point::new(4, 12), style).draw(&mut d).unwrap();
    }
    report("B8_eg_primitives", frames, t);

    println!("done");
    // Idle animation so a human can see the panel is alive.
    let mut f = 0i32;
    loop {
        d.blit::<Rgb565Be>(&full, frame, W).unwrap();
        spr.push_keyed(&mut d, Point::new((f * 2) % W as i32, H as i32 / 2 - 16), Rgb565::MAGENTA).unwrap();
        f += 1;
        delay.delay_millis(16);
    }
}
