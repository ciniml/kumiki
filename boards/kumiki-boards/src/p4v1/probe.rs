//! Board-detection probes for the ESP32-P4 rev v1 family. They run before
//! the board's drivers exist and only borrow the peripherals, so they work
//! on stolen handles that are dropped again.

use embedded_hal::i2c::I2c as _;
use kumiki_p4v1_hal::{
    Peripherals,
    gpio::AnyPin,
    i2c::{I2c, I2c0, I2c1},
};

/// Reads register `reg` of I²C device `addr` on controller `inst`
/// (`"i2c0"` / `"i2c1"`) with SDA / SCL on the given GPIOs, at 100 kHz.
/// `None` if nobody answers.
pub fn i2c_read_reg(_p: &mut Peripherals, inst: &str, sda: u8, scl: u8, addr: u8, reg: u8) -> Option<u8> {
    // SAFETY: `_p` is borrowed mutably, so nothing else uses the
    // controller or the pins while the probe runs; the handles are dropped
    // before returning, and the real driver reconfigures everything.
    let (sda, scl) = unsafe { (AnyPin::steal(sda), AnyPin::steal(scl)) };
    let mut i2c = match inst {
        "i2c0" => I2c::new(unsafe { I2c0::steal() }, sda, scl, 100_000),
        _ => I2c::new(unsafe { I2c1::steal() }, sda, scl, 100_000),
    };
    let mut b = [0u8];
    i2c.write_read(addr, &[reg], &mut b).ok().map(|_| b[0])
}
