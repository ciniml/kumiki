//! Everest ES7210 4-channel microphone ADC.
//!
//! Configures the ADC as an **I²S slave**: 16-bit, standard (Philips)
//! format, MIC1 and MIC2 on the left / right slots, MIC3 / MIC4 powered
//! down. The host supplies MCLK, BCK and LRCK; MCLK must run at
//! [`MCLK_MULTIPLE`] × fs, and must already be running when [`Es7210::init`]
//! is called.
//!
//! The register sequence is M5Unified 0.2.13's
//! (`_microphone_enabled_cb_cores3` / `_microphone_enabled_cb_tab5`, MIT),
//! verified on an M5Stack CoreS3 (ESP32-S3) and a Tab5 (ESP32-P4) at 48 kHz.
//!
//! Only `embedded_hal::i2c::I2c` is used: no waits, no allocation, no
//! panics (errors come back as [`Error`]).
#![no_std]
#![warn(missing_docs)]

use embedded_hal::i2c::I2c;

/// Default 7-bit address (AD0 = AD1 = 0).
pub const ADDR: u8 = 0x40;
/// MCLK / fs ratio the configuration expects.
pub const MCLK_MULTIPLE: u32 = 256;
/// Sample rates the configuration is meant for (the ADC runs off MCLK at
/// [`MCLK_MULTIPLE`] × fs; 48 kHz is the one verified on hardware).
pub const SAMPLE_RATES: &[u32] = &[8_000, 16_000, 22_050, 32_000, 44_100, 48_000];
/// Bits per sample on the I²S bus.
pub const BITS: u8 = 16;
/// Largest gain code ([`Es7210::set_gain`]); 0 is the least gain (see the
/// ES7210 datasheet, MICx_GAIN, for the dB values).
pub const MAX_GAIN_CODE: u8 = 14;
/// Gain code the initialisation uses (M5Unified's 0x1B, i.e. code 0x0B).
pub const DEFAULT_GAIN_CODE: u8 = 0x0B;

/// Driver error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    /// I²C failure.
    I2c(E),
    /// Argument out of range.
    Range,
}

/// One microphone input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mic {
    /// MIC1 (left slot).
    Mic1,
    /// MIC2 (right slot).
    Mic2,
}

/// M5Unified's sequence after the reset write (0x00 = 0xFF).
const INIT: [(u8, u8); 29] = [
    (0x00, 0x41), // RESET_CTL
    (0x01, 0x1F), // CLK_ON_OFF
    (0x06, 0x00), // DIGITAL_PDN
    (0x07, 0x20), // ADC_OSR
    (0x08, 0x10), // MODE_CFG
    (0x09, 0x30), // TCT0_CHPINI
    (0x0A, 0x30), // TCT1_CHPINI
    (0x20, 0x0A), // ADC34_HPF2
    (0x21, 0x2A), // ADC34_HPF1
    (0x22, 0x0A), // ADC12_HPF2
    (0x23, 0x2A), // ADC12_HPF1
    (0x02, 0xC1),
    (0x04, 0x01),
    (0x05, 0x00),
    (0x11, 0x60),
    (0x40, 0x42), // ANALOG_SYS
    (0x41, 0x70), // MICBIAS12
    (0x42, 0x70), // MICBIAS34
    (0x43, 0x10 | DEFAULT_GAIN_CODE), // MIC1_GAIN
    (0x44, 0x10 | DEFAULT_GAIN_CODE), // MIC2_GAIN
    (0x45, 0x00), // MIC3_GAIN
    (0x46, 0x00), // MIC4_GAIN
    (0x47, 0x00), // MIC1_LP
    (0x48, 0x00), // MIC2_LP
    (0x49, 0x00), // MIC3_LP
    (0x4A, 0x00), // MIC4_LP
    (0x4B, 0x00), // MIC12_PDN
    (0x4C, 0xFF), // MIC34_PDN
    (0x01, 0x14), // CLK_ON_OFF
];

/// ES7210 driver.
pub struct Es7210<I> {
    i2c: I,
    addr: u8,
}

impl<I: I2c> Es7210<I> {
    /// A driver for the chip at `addr` (normally [`ADDR`]).
    pub fn new(i2c: I, addr: u8) -> Self {
        Self { i2c, addr }
    }

    /// Releases the bus.
    pub fn release(self) -> I {
        self.i2c
    }

    fn write(&mut self, reg: u8, v: u8) -> Result<(), Error<I::Error>> {
        self.i2c.write(self.addr, &[reg, v]).map_err(Error::I2c)
    }

    /// Resets the chip and configures it (see the crate documentation).
    /// MCLK / BCK / LRCK must be running.
    pub fn init(&mut self) -> Result<(), Error<I::Error>> {
        self.write(0x00, 0xFF)?; // RESET_CTL: reset
        for (reg, v) in INIT {
            self.write(reg, v)?;
        }
        Ok(())
    }

    /// Sets the analogue gain of `mic` (`code` 0..=[`MAX_GAIN_CODE`]).
    pub fn set_gain(&mut self, mic: Mic, code: u8) -> Result<(), Error<I::Error>> {
        if code > MAX_GAIN_CODE {
            return Err(Error::Range);
        }
        let reg = match mic {
            Mic::Mic1 => 0x43,
            Mic::Mic2 => 0x44,
        };
        self.write(reg, 0x10 | code)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use std::vec::Vec;

    use super::*;

    #[derive(Default)]
    struct Rec(Vec<(u8, Vec<u8>)>);
    impl embedded_hal::i2c::ErrorType for Rec {
        type Error = core::convert::Infallible;
    }
    impl I2c for Rec {
        fn transaction(&mut self, addr: u8, ops: &mut [embedded_hal::i2c::Operation<'_>]) -> Result<(), Self::Error> {
            for op in ops {
                if let embedded_hal::i2c::Operation::Write(b) = op {
                    self.0.push((addr, b.to_vec()));
                }
            }
            Ok(())
        }
    }

    #[test]
    fn init_writes_m5unified_sequence() {
        let mut d = Es7210::new(Rec::default(), ADDR);
        d.init().unwrap();
        let w = d.release().0;
        assert_eq!(w.len(), 30);
        assert!(w.iter().all(|(a, b)| *a == ADDR && b.len() == 2));
        assert_eq!(w[0].1, [0x00, 0xFF]);
        assert_eq!(w[19].1, [0x43, 0x1B]); // MIC1 gain as M5Unified
        assert_eq!(w[29].1, [0x01, 0x14]);
    }

    #[test]
    fn gain_range() {
        let mut d = Es7210::new(Rec::default(), ADDR);
        d.set_gain(Mic::Mic2, 3).unwrap();
        assert_eq!(d.set_gain(Mic::Mic1, MAX_GAIN_CODE + 1), Err(Error::Range));
        assert_eq!(d.release().0, [(ADDR, std::vec![0x44, 0x13])]);
    }
}
