//! M4b: the CPU at 360 MHz, the 32 MiB PSRAM, and the MIPI-DSI display of
//! the M5Stack Tab5, with a test pattern (horizontal colour bands, a white
//! frame) and a moving rectangle, double-buffered in PSRAM. Logs the frame
//! rate once per second. Boot order as tab5-os: L2 line size, PSRAM, PVT
//! and clock, board (IO expander, panel identification), display.

#![no_std]
#![no_main]

mod tab5;

use kumiki_p4v1_hal::dsi::{self, Dsi, FB_BYTES, HEIGHT, WIDTH};
use kumiki_p4v1_hal::gpio::Output;
use kumiki_p4v1_hal::i2c::I2c;
use kumiki_p4v1_hal::ppa::{self, Block};
use kumiki_p4v1_hal::{self as hal, cache, clock, println, psram, time};

hal::entry!(main);

const fn rgb565(r: u32, g: u32, b: u32) -> u16 {
    (((r >> 3) << 11) | ((g >> 2) << 5) | (b >> 3)) as u16
}

const BANDS: [u16; 8] = [
    rgb565(255, 255, 255),
    rgb565(255, 255, 0),
    rgb565(0, 255, 255),
    rgb565(0, 255, 0),
    rgb565(255, 0, 255),
    rgb565(255, 0, 0),
    rgb565(0, 0, 255),
    rgb565(0, 0, 0),
];

/// The test pattern at (x, y): eight horizontal bands and a white frame
/// (shows the colour order, the orientation and cut-off edges).
fn pattern(x: usize, y: usize) -> u16 {
    if y < 8 || y >= HEIGHT - 8 || x < 8 || x >= WIDTH - 8 { 0xFFFF } else { BANDS[y * BANDS.len() / HEIGHT] }
}

fn pixels(fb: usize) -> &'static mut [u16] {
    unsafe { core::slice::from_raw_parts_mut(fb as *mut u16, WIDTH * HEIGHT) }
}

/// Redraws the pattern in rows `y0..y1`, columns `x0..x1`.
fn draw_pattern(fb: usize, x0: usize, y0: usize, x1: usize, y1: usize) {
    let px = pixels(fb);
    for y in y0..y1 {
        for x in x0..x1 {
            px[y * WIDTH + x] = pattern(x, y);
        }
    }
}

fn fill_rect(fb: usize, r: Rect, c: u16) {
    let px = pixels(fb);
    for y in r.y..r.y + RECT {
        px[y * WIDTH + r.x..y * WIDTH + r.x + RECT].fill(c);
    }
}

/// Writes rows `y0..y1` of `fb` back to PSRAM.
fn flush_rows(fb: usize, y0: usize, y1: usize) {
    dsi::flush(fb + y0 * WIDTH * 2, (y1 - y0) * WIDTH * 2);
}

const RECT: usize = 160;

#[derive(Clone, Copy)]
struct Rect {
    x: usize,
    y: usize,
}

fn ms_since(t: u64) -> u64 {
    (time::now() - t) / 16_000
}

fn us_since(t: u64) -> u64 {
    (time::now() - t) / 16
}

fn main() -> ! {
    println!();
    println!("kumiki-tab5-demo (kumiki-p4v1-hal)");
    println!("cpu at boot: {} MHz", hal::chip_info().cpu_hz / 1_000_000);

    // A previous program may have left the frame DMA reading PSRAM.
    if dsi::stop() {
        println!("display DMA was running; stopped");
    }
    cache::set_l2_line();
    println!("l2 line: {} bytes", cache::l2_line());

    let (base, size) = match psram::init() {
        Ok(v) => v,
        Err(e) => halt(e),
    };
    println!("psram: {} MiB at {base:#010x}", size >> 20);

    match clock::init_max() {
        Ok(mhz) => println!("clock: {mhz} MHz (measured {} MHz)", hal::chip_info().cpu_hz / 1_000_000),
        Err(e) => println!("clock: {e}"),
    }
    let bad = psram::memtest(size, 4096);
    println!("psram memtest at full clock: {}", if bad == 0 { "ok" } else { "FAILED" });

    let p = hal::take().unwrap();
    let mut i2c = I2c::new(p.i2c1, p.pins.gpio31, p.pins.gpio32, 100_000);
    let t = time::now();
    let panel = tab5::init(&mut i2c, p.pins.gpio23.degrade());
    println!("board ready: {panel:?} in {} ms", ms_since(t));
    let dsi_panel = match panel {
        tab5::Panel::Ili9881c { .. } => dsi::Panel::Ili9881c,
        tab5::Panel::St7123 => dsi::Panel::St7123,
        _ => halt("no supported panel"),
    };

    // Two framebuffers in the last 4 MiB of the PSRAM; the cache autoload
    // only below them (tab5-os doc/43 §4).
    let fbs = [base + size - (4 << 20), base + size - (4 << 20) + FB_BYTES];
    cache::autoload(fbs[0] - base);

    // fbs[1] by the CPU, then the PPA: a timed fill of fbs[0], and a copy
    // of fbs[1] into it.
    let t = time::now();
    draw_pattern(fbs[1], 0, 0, WIDTH, HEIGHT);
    dsi::flush(fbs[1], FB_BYTES);
    println!("cpu pattern draw + flush: {} ms", ms_since(t));
    ppa_test(fbs);

    let t = time::now();
    let mut d = match Dsi::init(dsi_panel, fbs[0]) {
        Ok(d) => d,
        Err(e) => halt(e),
    };
    let _backlight = Output::new(p.pins.gpio22, true);
    println!("display ready in {} ms, panel id {:x?}", ms_since(t), d.panel_id());
    let mut pm = [0u8; 1];
    match d.dcs_read(0x0A, &mut pm) {
        Ok(()) => println!("dcs 0x0a (power mode): {:#04x}", pm[0]),
        Err(e) => println!("dcs 0x0a read failed: {e}"),
    }

    // The animation: each buffer remembers where it has the rectangle.
    let mut old: [Option<Rect>; 2] = [None, None];
    let mut shown = 0usize;
    let mut swap_frame = d.frames();
    let (mut pos, mut vel) = ((40isize, 40isize), (7isize, 5isize));
    let colors = [rgb565(255, 128, 0), rgb565(128, 0, 255), rgb565(0, 160, 128)];
    let mut steps = 0u32;
    let mut last_report = time::now();
    let (mut last_frames, mut last_steps) = (d.frames(), 0u32);
    loop {
        d.service();
        if d.frames().wrapping_sub(swap_frame) >= 2 {
            let back = 1 - shown;
            let fb = fbs[back];
            // Next position, bouncing off the edges.
            for (p, v, max) in [(&mut pos.0, &mut vel.0, WIDTH - RECT), (&mut pos.1, &mut vel.1, HEIGHT - RECT)] {
                *p += *v;
                if *p < 0 || *p > max as isize {
                    *v = -*v;
                    *p = (*p).clamp(0, max as isize);
                }
            }
            let r = Rect { x: pos.0 as usize, y: pos.1 as usize };
            let (mut y0, mut y1) = (r.y, r.y + RECT);
            if let Some(o) = old[back] {
                draw_pattern(fb, o.x, o.y, o.x + RECT, o.y + RECT);
                y0 = y0.min(o.y);
                y1 = y1.max(o.y + RECT);
            }
            fill_rect(fb, r, colors[(steps as usize / 60) % colors.len()]);
            old[back] = Some(r);
            flush_rows(fb, y0, y1);
            d.show(fb);
            shown = back;
            swap_frame = d.frames();
            steps += 1;
        }
        if time::now() - last_report >= 1_000_000 * time::TICKS_PER_US {
            let el_ms = ms_since(last_report).max(1) as u32;
            let (f, s) = (d.frames(), steps);
            println!(
                "fps {} (frames {f}), animation {} steps/s, underruns {}, restarts {}",
                (f - last_frames) * 1000 / el_ms,
                (s - last_steps) * 1000 / el_ms,
                d.underruns(),
                d.restarts()
            );
            last_report = time::now();
            last_frames = f;
            last_steps = s;
        }
    }
}

/// Times a full-screen PPA fill of `fbs[0]` and a full-screen copy of
/// `fbs[1]` into it, and checks some pixels.
fn ppa_test(fbs: [usize; 2]) {
    let at = |base| Block { base, width: WIDTH as u32, height: HEIGHT as u32, x: 0, y: 0 };
    let (w, h) = (WIDTH as u32, HEIGHT as u32);
    // What the PPA writes: written back and dropped from the caches first.
    cache::writeback(fbs[0], FB_BYTES, true);
    cache::invalidate(fbs[0], FB_BYTES, true);
    let t = time::now();
    let r = ppa::fill(&at(fbs[0]), w, h, rgb565(0, 0, 255));
    let fill_us = us_since(t);
    let px = pixels(fbs[0]);
    let fill_ok = px[0] == rgb565(0, 0, 255) && px[WIDTH * HEIGHT - 1] == rgb565(0, 0, 255);
    println!("ppa fill 720x1280: {r:?} in {fill_us} us, check {}", if fill_ok { "ok" } else { "BAD" });

    cache::writeback(fbs[0], FB_BYTES, true);
    cache::invalidate(fbs[0], FB_BYTES, true);
    let t = time::now();
    let r = ppa::copy(&at(fbs[0]), &at(fbs[1]), w, h);
    let copy_us = us_since(t);
    let px = pixels(fbs[0]);
    let samples = [(0, 0), (100, 300), (360, 640), (719, 1279), (500, 1000)];
    let copy_ok = samples.iter().all(|&(x, y)| px[y * WIDTH + x] == pattern(x, y));
    println!("ppa copy 720x1280: {r:?} in {copy_us} us, check {}", if copy_ok { "ok" } else { "BAD" });
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
