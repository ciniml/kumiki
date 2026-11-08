//! Speaker smoke test for the Tab5: the generated `kumiki_boards::init`,
//! then three 0.4 s beeps through `Devices::speaker`, with the speaker
//! polled (`idle`) in between. (Used to find that the ES8388 must be set up
//! before the display power sequence.)

#![no_std]
#![no_main]

use kumiki_class::Speaker;
use kumiki_p4v1_hal::{self as hal, println, time};

hal::entry!(main);

fn idle_ms<S: Speaker>(s: &mut S, ms: u64) {
    let end = time::now_us() + ms * 1000;
    while time::now_us() < end {
        s.idle().ok();
        time::delay_us(5_000);
    }
}

fn beep<S: Speaker>(s: &mut S, n: u32) {
    println!("beep {n}");
    let mut chunk = [[0i16; 2]; 48];
    for (i, f) in chunk.iter_mut().enumerate() {
        let v = if i < 24 { (i as i32 * 1000 - 12000) as i16 } else { (36000 - i as i32 * 1000) as i16 };
        *f = [v, v];
    }
    for _ in 0..400 {
        s.play(&chunk).ok();
    }
    idle_ms(s, 800);
}

fn main() -> ! {
    println!();
    println!("kumiki-tab5-audio-bisect (generated init)");
    let p = hal::take().unwrap();
    let mut dev = match kumiki_boards::init(p) {
        Ok(d) => d,
        Err(e) => panic!("init: {e:?}"),
    };
    println!("speaker: {:?}", dev.speaker.as_ref().map(|s| s.name()));
    if let Some(s) = dev.speaker.as_mut() {
        idle_ms(s, 500);
        for n in 1..=3 {
            beep(s, n);
        }
    }
    println!("done");
    loop {
        if let Some(s) = dev.speaker.as_mut() {
            s.idle().ok();
        }
        dev.display.service();
        time::delay_us(5_000);
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("\n*** panic: {info}");
    loop {
        core::hint::spin_loop();
    }
}
