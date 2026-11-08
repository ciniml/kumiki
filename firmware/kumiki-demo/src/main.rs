//! kumiki demo: the same binary runs on M5Stack CoreS3, AtomS3 and StopWatch.
//! The demo itself is HAL-independent (`app.rs`, also built for the Tab5 by
//! `firmware-p4/kumiki-tab5-boards`); this file is the esp-hal entry point.
#![no_std]
#![no_main]

mod app;

use esp_backtrace as _;
use esp_hal::{clock::CpuClock, main};
pub(crate) use esp_println::println;

esp_bootloader_esp_idf::esp_app_desc!();

/// Time and delay for `app`.
mod platform {
    pub fn now_ms() -> u64 {
        esp_hal::time::Instant::now().duration_since_epoch().as_millis()
    }
    pub fn delay_ms(ms: u32) {
        esp_hal::delay::Delay::new().delay_millis(ms);
    }
}

#[main]
fn main() -> ! {
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::max()));
    match kumiki_boards::init(p) {
        Ok(dev) => app::run(dev),
        Err(e) => panic!("board init failed: {e:?}"),
    }
}
