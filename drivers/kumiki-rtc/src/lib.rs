//! I²C real-time clock drivers implementing [`kumiki_class::Rtc`].
//!
//! * [`Pcf8563`]: NXP PCF8563 and the register-compatible Belling BM8563
//!   (M5Stack CoreS3).
//! * [`Rx8130`]: Epson RX8130CE (M5Stack Tab5).
//!
//! Register use and the start-up writes follow M5Unified 0.2.13
//! (`src/utility/rtc/{PCF8563,RX8130}_Class.cpp`, MIT), which drives these
//! chips on the same boards.
#![no_std]
#![warn(missing_docs)]

use embedded_hal::i2c::I2c;
use kumiki_class::{DateTime, Rtc};

fn bcd(v: u8) -> u8 {
    (v >> 4) * 10 + (v & 0x0F)
}

fn to_bcd(v: u8) -> u8 {
    ((v / 10) << 4) | (v % 10)
}

/// NXP PCF8563 / Belling BM8563.
pub struct Pcf8563<I> {
    i2c: I,
    addr: u8,
}

impl<I: I2c> Pcf8563<I> {
    /// Default address.
    pub const ADDR: u8 = 0x51;

    /// A driver for the chip at `addr` (normally [`Self::ADDR`]).
    pub fn new(i2c: I, addr: u8) -> Self {
        Self { i2c, addr }
    }

    /// Starts the clock (control registers cleared) and turns the CLKOUT /
    /// timer source to 1/60 Hz, as M5Unified does. The first write is a
    /// dummy: some units ignore the first access after power-up.
    pub fn init(&mut self) -> Result<(), I::Error> {
        let _ = self.i2c.write(self.addr, &[0x00, 0x00]);
        self.i2c.write(self.addr, &[0x00, 0x00])?;
        self.i2c.write(self.addr, &[0x0E, 0x03])
    }
}

impl<I: I2c> Rtc for Pcf8563<I> {
    type Error = I::Error;

    fn datetime(&mut self) -> Result<DateTime, I::Error> {
        let mut b = [0u8; 7];
        self.i2c.write_read(self.addr, &[0x02], &mut b)?;
        Ok(DateTime {
            second: bcd(b[0] & 0x7F),
            minute: bcd(b[1] & 0x7F),
            hour: bcd(b[2] & 0x3F),
            day: bcd(b[3] & 0x3F),
            weekday: b[4] & 0x07,
            month: bcd(b[5] & 0x1F),
            // Century bit set = 19xx.
            year: bcd(b[6]) as u16 + if b[5] & 0x80 != 0 { 1900 } else { 2000 },
        })
    }

    fn set_datetime(&mut self, t: &DateTime) -> Result<(), I::Error> {
        let century = if t.year < 2000 { 0x80 } else { 0 };
        self.i2c.write(
            self.addr,
            &[
                0x02,
                to_bcd(t.second),
                to_bcd(t.minute),
                to_bcd(t.hour),
                to_bcd(t.day),
                t.weekday & 0x07,
                to_bcd(t.month) | century,
                to_bcd((t.year % 100) as u8),
            ],
        )
    }

    fn power_lost(&mut self) -> Result<bool, I::Error> {
        // VL bit of the seconds register.
        let mut b = [0u8];
        self.i2c.write_read(self.addr, &[0x02], &mut b)?;
        Ok(b[0] & 0x80 != 0)
    }
}

/// Epson RX8130CE.
pub struct Rx8130<I> {
    i2c: I,
    addr: u8,
}

impl<I: I2c> Rx8130<I> {
    /// Address.
    pub const ADDR: u8 = 0x32;

    /// A driver for the chip at `addr` (normally [`Self::ADDR`]).
    pub fn new(i2c: I, addr: u8) -> Self {
        Self { i2c, addr }
    }

    /// Start-up writes of M5Unified: set bits 0x30 of register 0x1F, clear
    /// register 0x30 and the control register 0x1E.
    pub fn init(&mut self) -> Result<(), I::Error> {
        let mut r = [0u8];
        self.i2c.write_read(self.addr, &[0x1F], &mut r)?;
        self.i2c.write(self.addr, &[0x1F, r[0] | 0x30])?;
        self.i2c.write(self.addr, &[0x30, 0x00])?;
        self.i2c.write(self.addr, &[0x1E, 0x00])
    }
}

impl<I: I2c> Rtc for Rx8130<I> {
    type Error = I::Error;

    fn datetime(&mut self) -> Result<DateTime, I::Error> {
        let mut b = [0u8; 7];
        self.i2c.write_read(self.addr, &[0x10], &mut b)?;
        Ok(DateTime {
            second: bcd(b[0] & 0x7F),
            minute: bcd(b[1] & 0x7F),
            hour: bcd(b[2] & 0x3F),
            // The weekday register is one-hot.
            weekday: if b[3] == 0 { 0 } else { b[3].trailing_zeros() as u8 },
            day: bcd(b[4] & 0x3F),
            month: bcd(b[5] & 0x1F),
            year: bcd(b[6]) as u16 + 2000,
        })
    }

    fn set_datetime(&mut self, t: &DateTime) -> Result<(), I::Error> {
        self.i2c.write(
            self.addr,
            &[
                0x10,
                to_bcd(t.second),
                to_bcd(t.minute),
                to_bcd(t.hour),
                1 << (t.weekday & 7),
                to_bcd(t.day),
                to_bcd(t.month),
                to_bcd((t.year % 100) as u8),
            ],
        )
    }

    fn power_lost(&mut self) -> Result<bool, I::Error> {
        // VBLF bit of the flag register.
        let mut b = [0u8];
        self.i2c.write_read(self.addr, &[0x1D], &mut b)?;
        Ok(b[0] & 0x80 != 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bcd_round_trip() {
        for v in 0..100u8 {
            assert_eq!(bcd(to_bcd(v)), v);
        }
        assert_eq!(to_bcd(59), 0x59);
    }
}
