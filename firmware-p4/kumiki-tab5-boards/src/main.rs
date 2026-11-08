//! The kumiki demo on the M5Stack Tab5: the board is brought up by
//! `kumiki_boards::init` (generated from `boards/dts/m5stack/tab5.dts`), and
//! the app is the very same source as the ESP32-S3 demo
//! (`firmware/kumiki-demo/src/app.rs`).

#![no_std]
#![no_main]

#[path = "../../../firmware/kumiki-demo/src/app.rs"]
mod app;

use kumiki_p4v1_hal as hal;
pub(crate) use kumiki_p4v1_hal::println;

hal::entry!(main);

/// Time and delay for `app`.
mod platform {
    use kumiki_p4v1_hal::time;
    pub fn now_ms() -> u64 {
        time::now_us() / 1000
    }
    pub fn delay_ms(ms: u32) {
        time::delay_us(ms as u64 * 1000);
    }
}

fn main() -> ! {
    println!();
    let p = hal::take().unwrap();
    match kumiki_boards::init(p) {
        Ok(dev) => app::run(dev),
        Err(e) => panic!("board init failed: {e:?}"),
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("\n*** panic: {info}");
    loop {
        core::hint::spin_loop();
    }
}
