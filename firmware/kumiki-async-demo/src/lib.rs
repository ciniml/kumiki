//! Executor-agnostic application code shared by the `blockon` and `embassy` binaries.
//!
//! Two futures:
//! * [`render_loop`] draws a frame into a RAM [`Sprite`] (CPU) and pushes it
//!   with `AsyncFastDraw::blit` (DMA, awaited via interrupt + waker);
//! * [`monitor`] spins a counter and yields; its rate shows how much CPU time
//!   is left while frames are on the wire, and it prints the statistics;
//! * [`audio_loop`] (boards with a speaker and a microphone) plays a quiet
//!   1 kHz beep with `AsyncSpeaker::play_async` while recording with
//!   `AsyncMicrophone::read_async`, and prints the rates and the 1 kHz level
//!   heard during the beep and during the silence.
//!
//! Nothing here depends on an executor, a time driver or an allocator.
#![no_std]

use core::{
    future::Future,
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    task::{Context, Poll},
};

use embedded_graphics::{
    mono_font::{MonoTextStyle, ascii::FONT_6X10},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, PrimitiveStyle},
    text::Text,
};
use esp_hal::time::Instant;
use esp_println::println;
use kumiki_boards::{AnyAsyncDisplay, AnyMicrophone, AnySpeaker};
use kumiki_class::{AsyncMicrophone, AsyncSpeaker, Microphone, Speaker};
use kumiki_gfx::{AsyncFastDraw, FastDraw, Rgb565Be, Sprite};

/// Largest panel of the family (CoreS3).
pub const MAX_W: usize = 320;
/// Largest panel of the family (CoreS3).
pub const MAX_H: usize = 240;

static mut FRAME: [u8; MAX_W * MAX_H * 2] = [0; MAX_W * MAX_H * 2];
static FRAME_TAKEN: AtomicBool = AtomicBool::new(false);
static FRAMES: AtomicU32 = AtomicU32::new(0);

/// One-shot "go" signal: the renderer sleeps (is not polled-and-spinning)
/// until the monitor has measured the idle baseline.
struct Go {
    set: AtomicBool,
    waker: critical_section::Mutex<core::cell::RefCell<Option<core::task::Waker>>>,
}

impl Go {
    const fn new() -> Self {
        Self { set: AtomicBool::new(false), waker: critical_section::Mutex::new(core::cell::RefCell::new(None)) }
    }
    fn fire(&self) {
        self.set.store(true, Ordering::Release);
        if let Some(w) = critical_section::with(|cs| self.waker.borrow_ref_mut(cs).take()) {
            w.wake();
        }
    }
    fn wait(&self) -> impl Future<Output = ()> + '_ {
        core::future::poll_fn(move |cx| {
            if self.set.load(Ordering::Acquire) {
                return Poll::Ready(());
            }
            critical_section::with(|cs| *self.waker.borrow_ref_mut(cs) = Some(cx.waker().clone()));
            if self.set.load(Ordering::Acquire) { Poll::Ready(()) } else { Poll::Pending }
        })
    }
}

static RENDER_GO: Go = Go::new();

/// The full-screen frame buffer (internal RAM). Panics if taken twice.
pub fn frame_buf() -> &'static mut [u8] {
    assert!(!FRAME_TAKEN.swap(true, Ordering::AcqRel), "frame buffer already taken");
    unsafe { &mut *core::ptr::addr_of_mut!(FRAME) }
}

/// Yield once to the executor.
pub fn yield_now() -> impl Future<Output = ()> {
    struct YieldNow(bool);
    impl Future for YieldNow {
        type Output = ();
        fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
            if self.0 {
                Poll::Ready(())
            } else {
                self.0 = true;
                cx.waker().wake_by_ref();
                Poll::Pending
            }
        }
    }
    YieldNow(false)
}

/// Render and push frames forever.
pub async fn render_loop(mut display: AnyAsyncDisplay, buf: &'static mut [u8]) -> ! {
    RENDER_GO.wait().await; // let the monitor measure the idle baseline first
    let size = AsyncFastDraw::size(&display);
    // Larger panels (StopWatch 468×468) get a centred MAX_W×MAX_H window; x is
    // kept even for controllers that need 2-pixel alignment.
    let (w, h) = ((size.width as i32).min(MAX_W as i32), (size.height as i32).min(MAX_H as i32));
    let origin = Point::new(((size.width as i32 - w) / 2) & !1, (size.height as i32 - h) / 2);
    let n = (w * h * 2) as usize;
    let mut spr = Sprite::<Rgb565Be, _>::new(&mut buf[..n], w as u16, h as u16).unwrap();
    let style = MonoTextStyle::new(&FONT_6X10, Rgb565::WHITE);
    let r = (w.min(h) / 6).max(8);
    let (mut x, mut y, mut vx, mut vy) = (r, r, 3, 2);
    loop {
        spr.clear(Rgb565::new(0, 6, 10)).ok();
        Circle::with_center(Point::new(x, y), (2 * r) as u32)
            .into_styled(PrimitiveStyle::with_fill(Rgb565::YELLOW))
            .draw(&mut spr)
            .ok();
        Text::new("kumiki async", Point::new(4, 12), style).draw(&mut spr).ok();
        spr.fill_rect(
            &embedded_graphics::primitives::Rectangle::new(Point::new(0, h - 4), Size::new((x * 2) as u32 % w as u32, 4)),
            Rgb565::GREEN,
        )
        .ok();
        // DMA push: while each chunk is on the wire, other futures run.
        spr.push_async(&mut display, origin).await.ok();
        FRAMES.fetch_add(1, Ordering::Relaxed);
        x += vx;
        y += vy;
        if x < r || x > w - r {
            vx = -vx;
        }
        if y < r || y > h - r {
            vy = -vy;
        }
    }
}

/// Count spins, report every second: frames/s and the share of CPU left over.
pub async fn monitor(label: &'static str) -> ! {
    // Baseline: spins per second with nothing else to do (after warming the
    // flash cache for 100 ms, then measured over 500 ms).
    let t0 = Instant::now();
    while t0.elapsed().as_millis() < 100 {
        yield_now().await;
    }
    let t0 = Instant::now();
    let mut base: u64 = 0;
    while t0.elapsed().as_millis() < 500 {
        base += 1;
        yield_now().await;
    }
    let base_per_s = base * 2;
    println!("[{label}] idle baseline: {base_per_s} spins/s");
    RENDER_GO.fire();

    let mut last = Instant::now();
    let mut spins: u64 = 0;
    loop {
        spins += 1;
        yield_now().await;
        if spins & 0x3FF == 0 && last.elapsed().as_millis() >= 1000 {
            let ms = last.elapsed().as_millis();
            let frames = FRAMES.swap(0, Ordering::Relaxed);
            let fps_x10 = frames as u64 * 10_000 / ms;
            // Spins of the other task while frames render/transfer, vs. the idle
            // baseline. The per-yield cost is executor-specific, so compare
            // within one executor only.
            let spins_per_s = spins * 1000 / ms;
            println!(
                "[{label}] {}.{} fps, other task: {} spins/s (idle baseline {})",
                fps_x10 / 10,
                fps_x10 % 10,
                spins_per_s,
                base_per_s
            );
            spins = 0;
            last = Instant::now();
        }
    }
}

/// Frames moved by [`audio_loop`] since the last report.
static TX_FRAMES: AtomicU32 = AtomicU32::new(0);
static RX_FRAMES: AtomicU32 = AtomicU32::new(0);
/// The player is in the beep part of its cycle.
static BEEP_ON: AtomicBool = AtomicBool::new(false);

/// Goertzel power of `freq` Hz in `x`, normalized by the window length.
fn goertzel(x: &[[i16; 2]], freq: f32, rate: f32) -> f32 {
    let w = 2.0 * core::f32::consts::PI * freq / rate;
    let c = 2.0 * libm_cos(w);
    let (mut s1, mut s2) = (0.0f32, 0.0f32);
    for f in x {
        let s = f[0] as f32 + c * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    let p = s1 * s1 + s2 * s2 - c * s1 * s2;
    p / (x.len() as f32 * x.len() as f32)
}

/// cos for the one Goertzel coefficient (no libm in this crate).
fn libm_cos(x: f32) -> f32 {
    // Taylor series around 0; |x| is small (2π·1k/48k ≈ 0.13).
    let x2 = x * x;
    1.0 - x2 / 2.0 + x2 * x2 / 24.0 - x2 * x2 * x2 / 720.0
}

/// Beep (200 ms of 1 kHz at 1/16 full scale) and silence (800 ms), queued
/// with `play_async`; record with `read_async`; report once per second.
pub async fn audio_loop(mut spk: AnySpeaker, mut mic: AnyMicrophone) -> ! {
    let rate = Speaker::sample_rate(&spk);
    let player = async {
        let mut chunk = [[0i16; 2]; 480]; // 10 ms at 48 kHz
        let mut phase = 0u32;
        let mut t = 0u32; // ms in the 1 s cycle
        let mut last = Instant::now();
        loop {
            let beep = t < 200;
            BEEP_ON.store(beep, Ordering::Relaxed);
            for f in chunk.iter_mut() {
                let v = if beep {
                    // A triangle wave is fine for a 1 kHz level check.
                    let period = (rate / 1000).max(4);
                    let x = (phase % period) as f32 / period as f32;
                    phase = phase.wrapping_add(1);
                    ((4.0 * (x - 0.5).abs() - 1.0) * 2048.0) as i16
                } else {
                    0
                };
                *f = [v, v];
            }
            spk.play_async(&chunk).await.ok();
            TX_FRAMES.fetch_add(chunk.len() as u32, Ordering::Relaxed);
            t = (t + 10) % 1000;
            if last.elapsed().as_millis() >= 1000 {
                last = Instant::now();
                REPORT.fire();
            }
        }
    };
    let recorder = async {
        let mut buf = [[0i16; 2]; 256];
        let mut win = [[0i16; 2]; 960];
        let (mut n, mut beep_lvl, mut quiet_lvl) = (0usize, 0.0f32, 0.0f32);
        loop {
            let got = mic.read_async(&mut buf).await.unwrap_or(0);
            RX_FRAMES.fetch_add(got as u32, Ordering::Relaxed);
            for f in &buf[..got] {
                win[n] = *f;
                n += 1;
                if n == win.len() {
                    n = 0;
                    let p = goertzel(&win, 1000.0, Microphone::sample_rate(&mic) as f32);
                    if BEEP_ON.load(Ordering::Relaxed) {
                        beep_lvl = beep_lvl.max(p);
                    } else {
                        quiet_lvl = quiet_lvl.max(p);
                    }
                }
            }
            if REPORT.take() {
                println!(
                    "[audio] tx {} frames/s, rx {} frames/s, 1 kHz level: beep {} / silence {}",
                    TX_FRAMES.swap(0, Ordering::Relaxed),
                    RX_FRAMES.swap(0, Ordering::Relaxed),
                    beep_lvl as u32,
                    quiet_lvl as u32
                );
                beep_lvl = 0.0;
                quiet_lvl = 0.0;
            }
        }
    };
    join(player, recorder).await
}

/// "Print a report" flag, set by the player once per second.
struct Flag(AtomicBool);
impl Flag {
    fn fire(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
    fn take(&self) -> bool {
        self.0.swap(false, Ordering::Relaxed)
    }
}
static REPORT: Flag = Flag(AtomicBool::new(false));

/// Poll two never-ending futures in turn.
async fn join<A: Future, B: Future>(a: A, b: B) -> ! {
    let mut a = core::pin::pin!(a);
    let mut b = core::pin::pin!(b);
    core::future::poll_fn(move |cx| {
        let _ = a.as_mut().poll(cx);
        let _ = b.as_mut().poll(cx);
        Poll::<()>::Pending
    })
    .await;
    unreachable!()
}
