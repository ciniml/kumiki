//! The same async demo on embassy (esp-rtos thread-mode executor): each
//! future becomes an embassy task. The application code is identical.
#![no_std]
#![no_main]

use embassy_executor::Spawner;
use esp_backtrace as _;
use esp_hal::{clock::CpuClock, main, timer::timg::TimerGroup};
use esp_println::println;
use esp_rtos::embassy::Executor;
use kumiki_async_demo::{frame_buf, monitor, render_loop};
use kumiki_boards::AnyAsyncDisplay;
use static_cell::StaticCell;

esp_bootloader_esp_idf::esp_app_desc!();

static EXECUTOR: StaticCell<Executor> = StaticCell::new();

#[embassy_executor::task]
async fn render_task(display: AnyAsyncDisplay, buf: &'static mut [u8]) {
    render_loop(display, buf).await
}

#[embassy_executor::task]
async fn monitor_task() {
    monitor("embassy").await
}

#[main]
fn main() -> ! {
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    let dev = kumiki_boards::init(p).unwrap_or_else(|e| panic!("board init failed: {e:?}"));
    println!("kumiki-async-demo (embassy): {}", dev.board.name());
    // The board definition hands back the peripherals it does not use.
    let timg0 = TimerGroup::new(dev.spare.timg0);
    esp_rtos::start(timg0.timer0, dev.spare.from_cpu_intr0);
    let display = dev.display.into_async();
    let buf = frame_buf();
    EXECUTOR.init(Executor::new()).run(move |spawner: Spawner| {
        spawner.spawn(render_task(display, buf).unwrap());
        spawner.spawn(monitor_task().unwrap());
    })
}
