//! The demo itself, written only against `kumiki-boards`' `Devices`, the
//! kumiki-class traits and kumiki-gfx: the same source runs on the ESP32-S3
//! family (esp-hal, `main.rs` here) and on the ESP32-P4 rev v1 family
//! (kumiki-p4v1-hal, `firmware-p4/kumiki-tab5-boards`).
//!
//! The including crate provides `crate::println!` and
//! `crate::platform::{now_ms, delay_ms}`.
//!
//! Button A (AtomS3: the screen button, CoreS3 / Tab5: touch anywhere,
//! StopWatch: A or B) cycles pages:
//! 0. board info + colour bars
//! 1. IMU readings (or "no IMU")
//! 2. keyed-sprite animation
//! 3. touch test (boards with touch)
//!
//! Pages 0–2 are drawn inside the display's *safe area* through a `Viewport`,
//! so they stay visible on StopWatch's round panel; the touch test uses full
//! panel coordinates.

use core::fmt::Write as _;

use embedded_graphics::{
    mono_font::{MonoFont, MonoTextStyleBuilder, ascii::FONT_6X10, ascii::FONT_10X20},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, PrimitiveStyle, Rectangle},
    text::Text,
};
use kumiki_boards::Devices;
use kumiki_class::{
    Backlight, ButtonKind, Buttons, DevicePm, Imu, Microphone, PmAction, Power, Rtc, Speaker, Touch, TouchPoint,
};
use kumiki_gfx::{FastDraw, Rgb565Be, Sprite, Viewport};

use crate::platform::{delay_ms, now_ms};
use crate::println;

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

/// Anything the demo draws on: the display itself or a `Viewport` of it.
trait Canvas: DrawTarget<Color = Rgb565> + FastDraw + OriginDimensions {}
impl<T: DrawTarget<Color = Rgb565> + FastDraw + OriginDimensions> Canvas for T {}

fn font_for<D: OriginDimensions>(d: &D) -> &'static MonoFont<'static> {
    if d.size().width >= 240 { &FONT_10X20 } else { &FONT_6X10 }
}

fn text<D: Canvas>(d: &mut D, s: &str, row: i32, color: Rgb565) {
    let font = font_for(d);
    let style = MonoTextStyleBuilder::new().font(font).text_color(color).background_color(Rgb565::BLACK).build();
    let h = font.character_size.height as i32;
    // Clear the rest of the line so shorter text does not leave stale glyphs.
    let w = d.size().width;
    d.fill_rect(&Rectangle::new(Point::new(0, row * h + 2), Size::new(w, h as u32)), Rgb565::BLACK).ok();
    Text::new(s, Point::new(2, row * h + 2 + font.baseline as i32), style).draw(d).ok();
}

fn draw_page0<D: Canvas>(d: &mut D, name: &str) {
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

/// A short triangle-wave beep (`hz`, `ms`) with a 5 ms fade in and out.
fn beep<S: Speaker>(s: &mut S, hz: u32, ms: u32) {
    let rate = s.sample_rate();
    let total = rate * ms / 1000;
    let fade = rate / 200;
    let period = (rate / hz).max(2);
    let mut chunk = [[0i16; 2]; 128];
    let mut n = 0;
    while n < total {
        let k = (total - n).min(chunk.len() as u32);
        for (i, f) in chunk[..k as usize].iter_mut().enumerate() {
            let t = n + i as u32;
            let ph = (t % period) as i32 * 4 * 12_000 / period as i32; // 0..48000
            let tri = if ph < 24_000 { ph - 12_000 } else { 36_000 - ph }; // ±12000
            let env = t.min(total - t).min(fade) as i32;
            let v = (tri * env / fade.max(1) as i32) as i16;
            *f = [v, v];
        }
        if s.play(&chunk[..k as usize]).is_err() {
            return;
        }
        n += k;
    }
}

/// cos(x) for |x| <= 1 (Taylor to x^8; no libm).
fn cos_small(x: f32) -> f32 {
    let x2 = x * x;
    1.0 - x2 / 2.0 * (1.0 - x2 / 12.0 * (1.0 - x2 / 30.0 * (1.0 - x2 / 56.0)))
}

fn isqrt(m: u64) -> u64 {
    let mut r = 0u64;
    while (r + 1) * (r + 1) <= m {
        r += 1;
    }
    r
}

/// Self-test tone amplitude below which the speaker counts as silent
/// (silent Tab5: about 5).
const SELFTEST_MIN: u32 = 100;

/// Audio self-test (the method of tab5-os' `spkcheck`): plays a 1 kHz tone
/// for `ms` while recording, and measures the 1 kHz component of the
/// recording (Goertzel, both channels, after the first 100 ms) against
/// everything else. Prints and returns (tone amplitude, total RMS) of the
/// louder channel; the tone is in sample units (full scale 32767).
fn audio_selftest<S: Speaker, M: Microphone>(s: &mut S, m: &mut M, ms: u32) -> (u32, u32) {
    const HZ: f32 = 1000.0;
    let rate = s.sample_rate();
    let w = 2.0 * core::f32::consts::PI * HZ / rate as f32;
    let k = 2.0 * cos_small(w);
    // Sine by rotation: sin((n+1)w) = k sin(nw) - sin((n-1)w).
    let (mut y1, mut y2) = (0.0f32, -(8000.0 * cos_small(core::f32::consts::FRAC_PI_2 - w)));
    let total = rate * ms / 1000;
    let skip = rate / 10;
    // Goertzel state per channel, and the sum of squares.
    let (mut g1, mut g2, mut sq, mut seen) = ([0.0f32; 2], [0.0f32; 2], [0u64; 2], 0u32);
    let mut out = [[0i16; 2]; 128];
    let mut inp = [[0i16; 2]; 256];
    // Drop what was recorded before.
    while let Ok(got) = m.read(&mut inp) {
        if got == 0 {
            break;
        }
    }
    let mut n = 0;
    // Keep reading for a while after the tone so the recording covers it.
    while n < total + rate / 20 {
        let c = out.len() as u32;
        for f in out.iter_mut() {
            let y = if n < total { k * y1 - y2 } else { 0.0 };
            y2 = y1;
            y1 = y;
            *f = [y as i16, y as i16];
        }
        if s.play(&out).is_err() {
            break;
        }
        n += c;
        while let Ok(got) = m.read(&mut inp) {
            if got == 0 {
                break;
            }
            for f in &inp[..got] {
                seen += 1;
                if seen <= skip || seen > total {
                    continue;
                }
                for ch in 0..2 {
                    let x = f[ch] as f32;
                    let g = x + k * g1[ch] - g2[ch];
                    g2[ch] = g1[ch];
                    g1[ch] = g;
                    sq[ch] += (f[ch] as i64 * f[ch] as i64) as u64;
                }
            }
        }
    }
    let used = seen.min(total).saturating_sub(skip).max(1) as f32;
    let mut best = (0u32, 0u32);
    for ch in 0..2 {
        let power = g1[ch] * g1[ch] + g2[ch] * g2[ch] - k * g1[ch] * g2[ch];
        // Amplitude of the component: 2 * sqrt(power) / N.
        let amp = isqrt((4.0 * power / (used * used)) as u64) as u32;
        let rms = isqrt(sq[ch] / used as u64) as u32;
        if amp >= best.0 {
            best = (amp, rms);
        }
    }
    println!("audio selftest: 1 kHz amplitude {} (rms {}) over {} frames recorded", best.0, best.1, used as u32);
    best
}

/// Speaker suspend / resume, checked through the microphone (doc/13): while
/// suspended the 1 kHz tone must not be heard, after resuming it must be.
fn speaker_pm_check<S: Speaker + DevicePm, M: Microphone>(s: &mut S, m: &mut M) {
    if let Err(e) = s.pm(PmAction::Suspend) {
        println!("speaker pm: suspend failed: {e:?}");
        return;
    }
    let quiet = audio_selftest(s, m, 300).0;
    if let Err(e) = s.pm(PmAction::Resume) {
        println!("speaker pm: resume failed: {e:?}");
        return;
    }
    let back = audio_selftest(s, m, 300).0;
    let ok = quiet < SELFTEST_MIN && back >= SELFTEST_MIN;
    println!(
        "speaker pm: suspended {quiet} (< {SELFTEST_MIN}), resumed {back} (>= {SELFTEST_MIN}): {}",
        if ok { "ok" } else { "FAILED" }
    );
}

/// Runs the demo on the brought-up board; never returns.
pub fn run(dev: Devices) -> ! {
    let Devices {
        board, boot, mut display, mut backlight, mut touch, mut imu, mut power, mut rtc, mut speaker, mut microphone, mut buttons, ..
    } = dev;
    println!("kumiki-demo: boot {:?} (warm: {})", boot, boot.is_warm());
    println!(
        "kumiki-demo: detected {} (touch: {}, imu: {}, power: {}, rtc: {}, speaker: {}, mic: {})",
        board.name(),
        touch.as_ref().map(|t| t.name()).unwrap_or("none"),
        imu.is_some(),
        power.is_some(),
        rtc.as_ref().map(|r| r.name()).unwrap_or("none"),
        speaker.as_ref().map(|s| s.name()).unwrap_or("none"),
        microphone.as_ref().map(|m| m.name()).unwrap_or("none")
    );
    // Microphone level: peak |sample| per channel since the last report.
    let (mut mic_peak, mut mic_frames, mut mic_shown) = ([0i32; 2], 0u32, [0i32; 2]);
    let (mut mic_sq, mut mic_t0) = ([0u64; 2], now_ms());
    // A failed self-test is retried once later, after restarting the speaker.
    let mut retest_at: Option<u64> = None;
    if let Some(s) = speaker.as_mut() {
        s.set_volume(96).ok();
        beep(s, 660, 80);
        beep(s, 990, 80);
        if let Some(m) = microphone.as_mut() {
            delay_ms(100);
            let mut ok = audio_selftest(s, m, 400).0 >= SELFTEST_MIN;
            if !ok {
                println!("audio selftest: no tone, restarting the speaker");
                s.restart().ok();
                ok = audio_selftest(s, m, 400).0 >= SELFTEST_MIN;
                if !ok {
                    retest_at = Some(now_ms() + 10_000);
                }
            }
            if ok {
                speaker_pm_check(s, m);
            }
        }
    }
    if let Some(i) = imu.as_mut() {
        // Suspend / resume round trip (the readings on page 1 show it came back).
        let r = i.pm(PmAction::Suspend).and_then(|_| i.pm(PmAction::Resume));
        println!("imu pm: suspend + resume {:?}", r);
    }
    if let Some(r) = rtc.as_mut() {
        println!("rtc: {:?}, power lost: {:?}", r.datetime(), r.power_lost());
    }
    backlight.set_brightness(200).ok();

    let mut sprite_buf = [0u8; 32 * 32 * 2];
    let mut spr = Sprite::<Rgb565Be, _>::new(&mut sprite_buf[..], 32, 32).unwrap();
    spr.clear(Rgb565::MAGENTA).ok();
    Circle::new(Point::new(1, 1), 30).into_styled(PrimitiveStyle::with_fill(Rgb565::YELLOW)).draw(&mut spr).ok();
    Circle::new(Point::new(9, 9), 6).into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK)).draw(&mut spr).ok();
    Circle::new(Point::new(18, 9), 6).into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK)).draw(&mut spr).ok();

    let mut btn = Buttons::default();
    let mut page = 0u8;
    // Boards with touch get a 4th page: touch calibration targets.
    let pages: u8 = if touch.is_some() { 4 } else { 3 };
    // Visible area: whole screen, or the square inscribed in a round panel.
    let safe = display.safe_area();
    println!("safe area: {:?}", safe);
    let mut redraw = true;
    let mut frame = 0i32;
    let (mut touch_ok, mut touch_err, mut touch_max) = (0u32, 0u32, 0usize);
    let (mut x, mut y, mut vx, mut vy) = (0i32, 0i32, 3i32, 2i32);

    loop {
        let now = now_ms();
        let raw = buttons.raw(touch.as_mut());
        btn.update(raw, now, |e| {
            if e.kind == ButtonKind::Click {
                page = (page + 1) % pages;
                redraw = true;
                println!("button {:?} click -> page {}", e.id, page);
            }
        });
        if let (Some(t), Some(s), Some(m)) = (retest_at, speaker.as_mut(), microphone.as_mut()) {
            if now >= t {
                retest_at = None;
                println!("audio selftest: retry, restarting the speaker");
                s.restart().ok();
                audio_selftest(s, m, 400);
            }
        }
        if let Some(s) = speaker.as_mut() {
            if redraw && frame > 0 {
                beep(s, 880 + 110 * page as u32, 40);
            }
            s.idle().ok();
        }
        if let Some(m) = microphone.as_mut() {
            let mut buf = [[0i16; 2]; 256];
            while let Ok(n) = m.read(&mut buf) {
                if n == 0 {
                    break;
                }
                for f in &buf[..n] {
                    mic_peak[0] = mic_peak[0].max((f[0] as i32).abs());
                    mic_peak[1] = mic_peak[1].max((f[1] as i32).abs());
                    mic_sq[0] += (f[0] as i64 * f[0] as i64) as u64;
                    mic_sq[1] += (f[1] as i64 * f[1] as i64) as u64;
                }
                mic_frames += n as u32;
            }
            if frame % 10 == 0 {
                mic_shown = mic_peak;
                if frame % 120 == 0 {
                    let ms = (now_ms() - mic_t0).max(1);
                    let rms = |s: u64| {
                        let m = s / mic_frames.max(1) as u64;
                        // Integer square root.
                        let mut r = 0u64;
                        while (r + 1) * (r + 1) <= m {
                            r += 1;
                        }
                        r
                    };
                    println!(
                        "mic: {} frames in {} ms ({} Hz), peak L {} R {}, rms L {} R {}",
                        mic_frames,
                        ms,
                        mic_frames as u64 * 1000 / ms,
                        mic_peak[0],
                        mic_peak[1],
                        rms(mic_sq[0]),
                        rms(mic_sq[1])
                    );
                    mic_frames = 0;
                    mic_sq = [0; 2];
                    mic_t0 = now_ms();
                }
                mic_peak = [0; 2];
            }
        }

        let size = display.size();
        if redraw && page != 3 {
            display.clear(Rgb565::BLACK).ok(); // also clears outside the safe area
        }
        match page {
            0 => {
                let mut v = Viewport::new(&mut display, safe);
                if redraw {
                    draw_page0(&mut v, board.name());
                }
                if frame % 30 == 0 {
                    if let Some(pw) = power.as_mut() {
                        let mut l = Line::<32>::new();
                        let mv = pw.battery_mv().ok().flatten().unwrap_or(0);
                        let usb = pw.vbus_present().ok().flatten().unwrap_or(false);
                        let _ = write!(l, "bat {}mV usb {}", mv, if usb { "yes" } else { "no" });
                        text(&mut v, l.as_str(), 3, Rgb565::new(20, 40, 31));
                    }
                }
                if frame % 15 == 0
                    && let Some(r) = rtc.as_mut()
                {
                    let mut l = Line::<32>::new();
                    match (r.datetime(), r.power_lost()) {
                        (Ok(t), Ok(lost)) => {
                            let _ = write!(
                                l,
                                "{:04}-{:02}-{:02} {:02}:{:02}:{:02}{}",
                                t.year,
                                t.month,
                                t.day,
                                t.hour,
                                t.minute,
                                t.second,
                                if lost { " ?" } else { "" }
                            );
                        }
                        _ => {
                            let _ = write!(l, "rtc read error");
                        }
                    }
                    text(&mut v, l.as_str(), 4, Rgb565::new(31, 50, 20));
                }
            }
            1 => {
                let mut v = Viewport::new(&mut display, safe);
                if redraw {
                    text(&mut v, "IMU", 0, Rgb565::new(31, 40, 0));
                }
                match imu.as_mut() {
                    Some(m) => {
                        if let (Ok(a), Ok(g)) = (m.accel(), m.gyro()) {
                            let mut l = Line::<32>::new();
                            let _ = write!(l, "ax {:+.2}", a.x);
                            text(&mut v, l.as_str(), 1, Rgb565::WHITE);
                            let mut l = Line::<32>::new();
                            let _ = write!(l, "ay {:+.2}", a.y);
                            text(&mut v, l.as_str(), 2, Rgb565::WHITE);
                            let mut l = Line::<32>::new();
                            let _ = write!(l, "az {:+.2}", a.z);
                            text(&mut v, l.as_str(), 3, Rgb565::WHITE);
                            let mut l = Line::<32>::new();
                            let _ = write!(l, "gz {:+.0}", g.z);
                            text(&mut v, l.as_str(), 4, Rgb565::new(16, 63, 16));
                        }
                    }
                    None => {
                        if redraw {
                            text(&mut v, "no IMU driver", 1, Rgb565::new(31, 20, 0));
                        }
                    }
                }
                if microphone.is_some() && frame % 10 == 0 {
                    // Level meter: peak of both channels, full scale = 32768.
                    let mut l = Line::<32>::new();
                    let _ = write!(l, "mic {:5} {:5}", mic_shown[0], mic_shown[1]);
                    text(&mut v, l.as_str(), 5, Rgb565::new(31, 40, 31));
                    let font_h = font_for(&v).character_size.height as i32;
                    let w = v.size().width as i32 - 4;
                    let y = 6 * font_h + 4;
                    let peak = mic_shown[0].max(mic_shown[1]);
                    let lit = (w * peak / 32768).clamp(0, w);
                    v.fill_rect(&Rectangle::new(Point::new(2, y), Size::new(lit as u32, 8)), Rgb565::GREEN).ok();
                    v.fill_rect(&Rectangle::new(Point::new(2 + lit, y), Size::new((w - lit) as u32, 8)), Rgb565::new(4, 8, 4))
                        .ok();
                }
            }
            3 => {
                // Touch test: crosshairs at the centre and ±(size/4) diagonally.
                if redraw {
                    display.clear(Rgb565::BLACK).ok();
                    let c = Point::new(size.width as i32 / 2, size.height as i32 / 2);
                    let d = size.width.min(size.height) as i32 / 4;
                    for t in [c, c - Point::new(d, d), c + Point::new(d, d)] {
                        let style = PrimitiveStyle::with_stroke(Rgb565::GREEN, 1);
                        embedded_graphics::primitives::Line::new(t - Point::new(10, 0), t + Point::new(10, 0))
                            .into_styled(style)
                            .draw(&mut display)
                            .ok();
                        embedded_graphics::primitives::Line::new(t - Point::new(0, 10), t + Point::new(0, 10))
                            .into_styled(style)
                            .draw(&mut display)
                            .ok();
                        let mut l = Line::<16>::new();
                        let _ = write!(l, "{},{}", t.x, t.y);
                        let style = MonoTextStyleBuilder::new().font(&FONT_6X10).text_color(Rgb565::GREEN).build();
                        Text::new(l.as_str(), t + Point::new(12, -4), style).draw(&mut display).ok();
                    }
                }
            }
            _ => {
                let mut v = Viewport::new(&mut display, safe);
                let vs = v.size();
                if redraw {
                    v.clear(Rgb565::new(0, 8, 12)).ok();
                }
                // Erase the old position, move, draw keyed sprite.
                v.fill_rect(&Rectangle::new(Point::new(x, y), Size::new(32, 32)), Rgb565::new(0, 8, 12)).ok();
                x += vx;
                y += vy;
                if x < 0 || x + 32 > vs.width as i32 {
                    vx = -vx;
                    x += 2 * vx;
                }
                if y < 0 || y + 32 > vs.height as i32 {
                    vy = -vy;
                    y += 2 * vy;
                }
                spr.push_keyed(&mut v, Point::new(x, y), Rgb565::MAGENTA).ok();
                // A second copy spinning and breathing in the middle
                // (`Sprite::push_rotate_zoom`); its square is cleared first.
                let zoom_max = (vs.width.min(vs.height) as f32 / 64.0).clamp(1.0, 6.0);
                let phase = (frame % 120) as f32 / 120.0;
                let zoom = zoom_max * (0.6 + 0.4 * (1.0 - (2.0 * phase - 1.0).abs()));
                let c = Point::new(vs.width as i32 / 2, vs.height as i32 / 2);
                let r = (32.0 * zoom_max * 0.72) as i32 + 2;
                v.fill_rect(&Rectangle::new(c - Point::new(r, r), Size::new(2 * r as u32, 2 * r as u32)), Rgb565::new(0, 8, 12))
                    .ok();
                spr.push_rotate_zoom(&mut v, c, (frame * 3) as f32, zoom, zoom, Some(Rgb565::MAGENTA)).ok();
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
                    Circle::with_center(display.touch_to_screen(p), 9)
                        .into_styled(PrimitiveStyle::with_fill(Rgb565::RED))
                        .draw(&mut display)
                        .ok();
                }
                if n > 0 && frame % 10 == 0 {
                    println!("touch: {} point(s), first at ({}, {})", n, pts[0].x, pts[0].y);
                }
                if n > 0 && page == 3 {
                    // Show the reported coordinate in the middle (readable on round panels).
                    let c = Point::new(size.width as i32 / 2, size.height as i32 / 2);
                    let mut l = Line::<24>::new();
                    let s = display.touch_to_screen(&pts[0]);
                    let _ = write!(l, "touch {:4},{:4}", s.x, s.y);
                    let style = MonoTextStyleBuilder::new()
                        .font(font_for(&display))
                        .text_color(Rgb565::YELLOW)
                        .background_color(Rgb565::BLACK)
                        .build();
                    Text::new(l.as_str(), c + Point::new(-75, 40), style).draw(&mut display).ok();
                }
            }
        }

        display.flush().ok(); // frame-buffered displays (StopWatch, Tab5) present now
        redraw = false;
        frame += 1;
        delay_ms(16);
    }
}
