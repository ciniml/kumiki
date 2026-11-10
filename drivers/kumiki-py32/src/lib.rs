//! M5Stack PY32 IO expander: a PY32 MCU on I²C with 14 GPIO lines, an ADC,
//! PWM and an addressable RGB LED driver. On the Stack-chan base for the
//! CoreS3 (address 0x6F) line 0 switches the servo 5 V supply (VM_EN) and
//! line 13 carries the data of the 12 RGB LEDs.
//!
//! The register map and the LED format follow StackChan-BSP
//! (`drivers/PY32IOExpander`, `M5StackChan::io_expander_init`, MIT,
//! © 2026 M5Stack Technology). GPIO and LEDs only; no ADC / PWM / interrupts.
//!
//! The chip boots slowly (up to about 1.2 s after power-on): until then the
//! version register reads 0x00 / 0xFF or the address does not answer
//! ([`Py32::is_ready`]).
#![no_std]
#![warn(missing_docs)]

use embedded_hal::i2c::I2c;

/// Address on the Stack-chan base.
pub const ADDR: u8 = 0x6F;
/// Number of GPIO lines.
pub const LINES: u8 = 14;
/// Most LEDs the driver handles.
pub const MAX_LEDS: u8 = 32;

/// Registers. Line registers come in pairs: L = lines 0–7, H = lines 8–13.
pub mod reg {
    /// Firmware version (0x00 / 0xFF: not ready).
    pub const VERSION: u8 = 0x02;
    /// Direction, 1 = output.
    pub const MODE_L: u8 = 0x03;
    /// Output level.
    pub const OUT_L: u8 = 0x05;
    /// Input level.
    pub const IN_L: u8 = 0x07;
    /// Pull-up enable.
    pub const PU_L: u8 = 0x09;
    /// Pull-down enable.
    pub const PD_L: u8 = 0x0B;
    /// Drive, 1 = open drain.
    pub const DRV_L: u8 = 0x13;
    /// LED count (bits 5:0); bit 6 latches the LED RAM.
    pub const LED_CFG: u8 = 0x24;
    /// LED RAM: RGB565, little-endian, 2 bytes per LED.
    pub const LED_RAM: u8 = 0x30;
}

/// Driver error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    /// I²C failure.
    I2c(E),
    /// Line or LED index out of range.
    Range,
}

impl<E> From<E> for Error<E> {
    fn from(e: E) -> Self {
        Error::I2c(e)
    }
}

/// How a line is set up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineMode {
    /// Push-pull output, starting at the given level (pull-up enabled, as
    /// StackChan-BSP does for its outputs).
    Output(bool),
    /// Input with the pull-up enabled.
    InputPullUp,
    /// Input, no pulls (high impedance).
    Input,
}

/// RGB888 → RGB565.
pub const fn rgb565(r: u8, g: u8, b: u8) -> u16 {
    ((r as u16 & 0xF8) << 8) | ((g as u16 & 0xFC) << 3) | (b as u16 >> 3)
}

/// Register and bit of `line` in a register pair starting at `base`.
fn bit(base: u8, line: u8) -> (u8, u8) {
    if line < 8 { (base, 1 << line) } else { (base + 1, 1 << (line - 8)) }
}

fn read<I: I2c>(i2c: &mut I, addr: u8, r: u8) -> Result<u8, I::Error> {
    let mut b = [0];
    i2c.write_read(addr, &[r], &mut b)?;
    Ok(b[0])
}

fn update<I: I2c>(i2c: &mut I, addr: u8, base: u8, line: u8, on: bool) -> Result<(), I::Error> {
    let (r, m) = bit(base, line);
    let v = read(i2c, addr, r)?;
    let n = if on { v | m } else { v & !m };
    if n != v { i2c.write(addr, &[r, n]) } else { Ok(()) }
}

/// The expander.
pub struct Py32<I> {
    i2c: I,
    addr: u8,
}

impl<I: I2c> Py32<I> {
    /// A driver for the chip at `addr` (no bus traffic).
    pub fn new(i2c: I, addr: u8) -> Self {
        Self { i2c, addr }
    }

    /// Firmware version.
    pub fn version(&mut self) -> Result<u8, Error<I::Error>> {
        Ok(read(&mut self.i2c, self.addr, reg::VERSION)?)
    }

    /// Whether the chip has booted (answers with a version other than 0x00 / 0xFF).
    pub fn is_ready(&mut self) -> bool {
        matches!(self.version(), Ok(v) if v != 0x00 && v != 0xFF)
    }

    /// Set up `line`.
    pub fn set_line_mode(&mut self, line: u8, mode: LineMode) -> Result<(), Error<I::Error>> {
        if line >= LINES {
            return Err(Error::Range);
        }
        let (i2c, a) = (&mut self.i2c, self.addr);
        match mode {
            LineMode::Output(level) => {
                // Level first, so the line does not glitch when it turns output.
                update(i2c, a, reg::OUT_L, line, level)?;
                update(i2c, a, reg::PD_L, line, false)?;
                update(i2c, a, reg::PU_L, line, true)?;
                update(i2c, a, reg::DRV_L, line, false)?;
                update(i2c, a, reg::MODE_L, line, true)?;
            }
            LineMode::InputPullUp | LineMode::Input => {
                update(i2c, a, reg::MODE_L, line, false)?;
                update(i2c, a, reg::PD_L, line, false)?;
                update(i2c, a, reg::PU_L, line, mode == LineMode::InputPullUp)?;
            }
        }
        Ok(())
    }

    /// Drive an output line.
    pub fn set_line(&mut self, line: u8, level: bool) -> Result<(), Error<I::Error>> {
        if line >= LINES {
            return Err(Error::Range);
        }
        Ok(update(&mut self.i2c, self.addr, reg::OUT_L, line, level)?)
    }

    /// Read a line's input level.
    pub fn line(&mut self, line: u8) -> Result<bool, Error<I::Error>> {
        if line >= LINES {
            return Err(Error::Range);
        }
        let (r, m) = bit(reg::IN_L, line);
        Ok(read(&mut self.i2c, self.addr, r)? & m != 0)
    }

    /// Number of LEDs on the LED line (StackChan-BSP: 12).
    pub fn set_led_count(&mut self, n: u8) -> Result<(), Error<I::Error>> {
        if n > MAX_LEDS {
            return Err(Error::Range);
        }
        Ok(self.i2c.write(self.addr, &[reg::LED_CFG, n & 0x3F])?)
    }

    /// Write LED colors (RGB565) from `first` on; shown at [`Py32::show_leds`].
    pub fn set_leds(&mut self, first: u8, colors: &[u16]) -> Result<(), Error<I::Error>> {
        if first as usize + colors.len() > MAX_LEDS as usize {
            return Err(Error::Range);
        }
        let mut buf = [0u8; 1 + 2 * MAX_LEDS as usize];
        buf[0] = reg::LED_RAM + first * 2;
        for (k, c) in colors.iter().enumerate() {
            buf[1 + 2 * k..3 + 2 * k].copy_from_slice(&c.to_le_bytes());
        }
        Ok(self.i2c.write(self.addr, &buf[..1 + 2 * colors.len()])?)
    }

    /// Latch the LED RAM onto the LEDs.
    pub fn show_leds(&mut self) -> Result<(), Error<I::Error>> {
        let v = read(&mut self.i2c, self.addr, reg::LED_CFG)?;
        Ok(self.i2c.write(self.addr, &[reg::LED_CFG, v | 0x40])?)
    }

    /// Release the bus handle.
    pub fn release(self) -> I {
        self.i2c
    }
}

/// One output line as an embedded-hal `OutputPin` (read-modify-write of
/// the output register; the line must already be an output, see
/// [`Py32::set_line_mode`]).
pub struct Py32Pin<I> {
    i2c: I,
    addr: u8,
    line: u8,
}

impl<I: I2c> Py32Pin<I> {
    /// Line `line` of the chip at `addr`.
    pub fn new(i2c: I, addr: u8, line: u8) -> Self {
        Self { i2c, addr, line }
    }
}

/// Error of [`Py32Pin`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PinError;

impl embedded_hal::digital::Error for PinError {
    fn kind(&self) -> embedded_hal::digital::ErrorKind {
        embedded_hal::digital::ErrorKind::Other
    }
}

impl<I: I2c> embedded_hal::digital::ErrorType for Py32Pin<I> {
    type Error = PinError;
}

impl<I: I2c> embedded_hal::digital::OutputPin for Py32Pin<I> {
    fn set_low(&mut self) -> Result<(), PinError> {
        update(&mut self.i2c, self.addr, reg::OUT_L, self.line, false).map_err(|_| PinError)
    }
    fn set_high(&mut self) -> Result<(), PinError> {
        update(&mut self.i2c, self.addr, reg::OUT_L, self.line, true).map_err(|_| PinError)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use embedded_hal::digital::OutputPin;
    use std::vec::Vec;

    /// A register file behind a fake bus.
    struct Regs {
        r: [u8; 0x80],
        writes: Vec<Vec<u8>>,
    }

    impl embedded_hal::i2c::ErrorType for Regs {
        type Error = core::convert::Infallible;
    }

    impl I2c for Regs {
        fn transaction(&mut self, _addr: u8, ops: &mut [embedded_hal::i2c::Operation<'_>]) -> Result<(), Self::Error> {
            let mut ptr = 0usize;
            for op in ops {
                match op {
                    embedded_hal::i2c::Operation::Write(b) => {
                        ptr = b[0] as usize;
                        for (k, v) in b[1..].iter().enumerate() {
                            self.r[ptr + k] = *v;
                        }
                        if b.len() > 1 {
                            self.writes.push(b.to_vec());
                        }
                    }
                    embedded_hal::i2c::Operation::Read(b) => {
                        for (k, v) in b.iter_mut().enumerate() {
                            *v = self.r[ptr + k];
                        }
                    }
                }
            }
            Ok(())
        }
    }

    fn chip() -> Py32<Regs> {
        Py32::new(Regs { r: [0; 0x80], writes: Vec::new() }, ADDR)
    }

    #[test]
    fn output_line_sets_level_pull_up_push_pull_then_direction() {
        let mut c = chip();
        c.set_line_mode(13, LineMode::Output(true)).unwrap();
        let r = &c.i2c.r;
        assert_eq!(r[(reg::OUT_L + 1) as usize], 1 << 5);
        assert_eq!(r[(reg::PU_L + 1) as usize], 1 << 5);
        assert_eq!(r[(reg::MODE_L + 1) as usize], 1 << 5);
        // The direction is written last.
        assert_eq!(c.i2c.writes.last().unwrap()[0], reg::MODE_L + 1);
    }

    #[test]
    fn readiness_rejects_booting_values() {
        let mut c = chip();
        assert!(!c.is_ready());
        c.i2c.r[reg::VERSION as usize] = 0xFF;
        assert!(!c.is_ready());
        c.i2c.r[reg::VERSION as usize] = 0x11;
        assert!(c.is_ready());
    }

    #[test]
    fn leds_are_little_endian_rgb565_and_latch_keeps_the_count() {
        let mut c = chip();
        c.set_led_count(12).unwrap();
        c.set_leds(1, &[rgb565(255, 0, 0), 0x1234]).unwrap();
        assert_eq!(&c.i2c.r[0x32..0x36], &[0x00, 0xF8, 0x34, 0x12]);
        c.show_leds().unwrap();
        assert_eq!(c.i2c.r[reg::LED_CFG as usize], 12 | 0x40);
        assert_eq!(c.set_leds(31, &[0, 0]), Err(Error::Range));
    }

    #[test]
    fn pin_drives_only_its_bit() {
        let mut c = chip();
        c.i2c.r[reg::OUT_L as usize] = 0b1000_0000;
        let mut p = Py32Pin::new(c.release(), ADDR, 0);
        p.set_high().unwrap();
        assert_eq!(p.i2c.r[reg::OUT_L as usize], 0b1000_0001);
        p.set_low().unwrap();
        assert_eq!(p.i2c.r[reg::OUT_L as usize], 0b1000_0000);
    }
}
