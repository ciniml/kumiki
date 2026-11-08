//! The async demo on a ~40-line hand-written executor: no embassy, no RTOS,
//! no time driver. Proves kumiki's async path is executor-agnostic.
#![no_std]
#![no_main]

use core::{
    future::Future,
    pin::pin,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll, RawWaker, RawWakerVTable, Waker},
};

use esp_backtrace as _;
use esp_hal::{clock::CpuClock, main};
use esp_println::println;
use kumiki_async_demo::{frame_buf, monitor, render_loop};

esp_bootloader_esp_idf::esp_app_desc!();

static WOKEN: AtomicBool = AtomicBool::new(true);

fn waker() -> Waker {
    fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(core::ptr::null(), &VTABLE)
    }
    fn wake(_: *const ()) {
        WOKEN.store(true, Ordering::Release);
    }
    fn drop(_: *const ()) {}
    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake, drop);
    unsafe { Waker::from_raw(RawWaker::new(core::ptr::null(), &VTABLE)) }
}

/// Poll `fut` whenever something woke it (interrupt handlers call `wake`).
fn block_on<F: Future>(fut: F) -> F::Output {
    let mut fut = pin!(fut);
    let waker = waker();
    let mut cx = Context::from_waker(&waker);
    loop {
        if WOKEN.swap(false, Ordering::AcqRel) {
            if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
                return v;
            }
        }
    }
}

/// Run two futures concurrently until both complete (ours never do).
async fn join2<A: Future, B: Future>(a: A, b: B) {
    let mut a = pin!(a);
    let mut b = pin!(b);
    let (mut a_done, mut b_done) = (false, false);
    core::future::poll_fn(move |cx| {
        if !a_done {
            a_done = a.as_mut().poll(cx).is_ready();
        }
        if !b_done {
            b_done = b.as_mut().poll(cx).is_ready();
        }
        if a_done && b_done { Poll::Ready(()) } else { Poll::Pending }
    })
    .await
}

#[main]
fn main() -> ! {
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let dev = kumiki_boards::init(p).unwrap_or_else(|e| panic!("board init failed: {e:?}"));
    println!("kumiki-async-demo (hand-written block_on): {}", dev.board.name());
    let display = dev.display.into_async();
    block_on(join2(render_loop(display, frame_buf()), monitor("block_on")));
    unreachable!()
}
