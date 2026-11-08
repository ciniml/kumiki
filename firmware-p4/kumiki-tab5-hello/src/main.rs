//! M4a: boot log, chip information and a scan of the Tab5's internal I²C
//! bus (SDA = GPIO31, SCL = GPIO32).
//!
//! Expected on the Tab5 (tab5-os doc/06): PI4IOE5V6408 ×2 (0x43, 0x44),
//! GT911 touch (0x14 or 0x5D), BMI270 (0x68), RX8130 RTC (0x32),
//! INA226 (0x41), ES8388 (0x10), ES7210 (0x40) ... The scan only checks for
//! an ACK; nothing is written.

#![no_std]
#![no_main]

use embedded_hal::i2c::I2c as _;
use kumiki_p4v1_hal::{self as hal, i2c::I2c, println, time};

hal::entry!(main);

fn main() -> ! {
    println!();
    println!("kumiki-tab5-hello (kumiki-p4v1-hal)");
    let info = hal::chip_info();
    let m = info.mac;
    println!(
        "misa {:#010x}, cpu {} MHz, mac {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
        info.misa,
        info.cpu_hz / 1_000_000,
        m[0],
        m[1],
        m[2],
        m[3],
        m[4],
        m[5]
    );

    let p = hal::take().unwrap();
    let mut i2c = I2c::new(p.i2c1, p.pins.gpio31, p.pins.gpio32, 100_000);
    let mut found = 0;
    hal::print!("i2c1 scan:");
    for addr in 0x08u8..0x78 {
        if i2c.write(addr, &[]).is_ok() {
            hal::print!(" {addr:#04x}");
            found += 1;
        }
    }
    println!(" ({found} devices)");

    // PI4IOE5V6408 #1 (0x43): read the device ID / control register (0x01).
    let mut id = [0u8; 1];
    match i2c.write_read(0x43, &[0x01], &mut id) {
        Ok(()) => println!("pi4ioe@0x43 reg1 = {:#04x}", id[0]),
        Err(e) => println!("pi4ioe@0x43 read failed: {e:?}"),
    }

    let mut n = 0u32;
    loop {
        time::delay_us(1_000_000);
        n += 1;
        println!("alive {n} s (t = {} us)", time::now_us());
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("\n*** panic: {info}");
    loop {
        core::hint::spin_loop();
    }
}
