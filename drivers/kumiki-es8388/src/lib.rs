//! Everest ES8388 audio codec, DAC (playback) side.
//!
//! Configures the codec as an **I²S slave**: 16-bit, standard (Philips)
//! format, the DAC on LOUT1 / ROUT1 / LOUT2 / ROUT2, the ADC powered down.
//! The host supplies MCLK, BCK and LRCK; MCLK at [`MCLK_MULTIPLE`] × fs,
//! already running when [`Es8388::init`] is called. A speaker amplifier
//! behind the outputs is **not** handled here: enable it separately (on the
//! M5Stack Tab5, IO expander #1 line 1).
//!
//! The register sequence is M5Unified 0.2.13's `_speaker_enabled_cb_tab5`
//! (MIT), verified on a Tab5 (ESP32-P4) at 48 kHz. On the Tab5 the codec
//! must be configured before the display power sequence, or it stays silent
//! (found on hardware; cause unknown).
//!
//! Only `embedded_hal::i2c::I2c` is used: no waits, no allocation, no
//! panics (errors come back as [`Error`]).
#![no_std]
#![warn(missing_docs)]

use embedded_hal::i2c::I2c;

/// Default 7-bit address (CE = 0).
pub const ADDR: u8 = 0x10;
/// MCLK / fs ratio the configuration expects.
pub const MCLK_MULTIPLE: u32 = 256;
/// Sample rates the configuration is meant for (48 kHz is the one verified
/// on hardware).
pub const SAMPLE_RATES: &[u32] = &[8_000, 16_000, 22_050, 32_000, 44_100, 48_000];
/// Bits per sample on the I²S bus.
pub const BITS: u8 = 16;
/// Largest DAC attenuation in 0.5 dB steps ([`Es8388::set_attenuation`]):
/// 192 = -96 dB.
pub const MAX_ATTENUATION: u8 = 0xC0;

/// Driver error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    /// I²C failure.
    I2c(E),
    /// Argument out of range.
    Range,
}

const DACCONTROL3: u8 = 25;
const DAC_MUTE: u8 = 1 << 2;
const LDACVOL: u8 = 26;
const RDACVOL: u8 = 27;

/// M5Unified's sequence.
const INIT: [(u8, u8); 28] = [
    (0, 0x80), // RESET / CSM power on
    (0, 0x00),
    (0, 0x00),
    (0, 0x0E),
    (1, 0x00),
    (2, 0x0A),  // chip power: power up all
    (3, 0xFF),  // ADC power: power down all
    (4, 0x3C),  // DAC power: power up, LOUT1/ROUT1/LOUT2/ROUT2 enable
    (5, 0x00),  // ChipLowPower1
    (6, 0x00),  // ChipLowPower2
    (7, 0x7C),  // VSEL
    (8, 0x00),  // I2S slave mode
    (23, 0x18), // I2S format (16 bit)
    (24, 0x00), // MCLK ratio
    (DACCONTROL3, 0x20), // DAC unmute
    (LDACVOL, 0x00),     // 0 dB
    (RDACVOL, 0x00),     // 0 dB
    (28, 0x08), // digital click-free power up / down
    (29, 0x00),
    (38, 0x00), // DAC CTRL16
    (39, 0xB8), // left mixer
    (42, 0xB8), // right mixer
    (43, 0x08), // ADC and DAC separate
    (45, 0x00), // 1.5k VREF analog output
    (46, 0x21),
    (47, 0x21),
    (48, 0x21),
    (49, 0x21),
];

/// ES8388 driver (DAC side).
pub struct Es8388<I> {
    i2c: I,
    addr: u8,
}

impl<I: I2c> Es8388<I> {
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

    /// Resets and configures the codec for playback (see the crate
    /// documentation), unmuted at 0 dB. MCLK / BCK / LRCK must be running.
    pub fn init(&mut self) -> Result<(), Error<I::Error>> {
        for (reg, v) in INIT {
            self.write(reg, v)?;
        }
        Ok(())
    }

    /// DAC attenuation in 0.5 dB steps, both channels (0 = 0 dB …
    /// [`MAX_ATTENUATION`] = -96 dB).
    pub fn set_attenuation(&mut self, half_db: u8) -> Result<(), Error<I::Error>> {
        if half_db > MAX_ATTENUATION {
            return Err(Error::Range);
        }
        self.write(LDACVOL, half_db)?;
        self.write(RDACVOL, half_db)
    }

    /// Volume 0..=100: 100 = 0 dB, each step 0.5 dB less (0 = -50 dB); use
    /// [`set_mute`](Self::set_mute) for silence.
    pub fn set_volume(&mut self, percent: u8) -> Result<(), Error<I::Error>> {
        if percent > 100 {
            return Err(Error::Range);
        }
        self.set_attenuation(100 - percent)
    }

    /// Mutes or unmutes the DAC (soft ramp, `DACCONTROL3.DACMute`).
    pub fn set_mute(&mut self, mute: bool) -> Result<(), Error<I::Error>> {
        self.write(DACCONTROL3, if mute { 0x20 | DAC_MUTE } else { 0x20 })
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
        let mut d = Es8388::new(Rec::default(), ADDR);
        d.init().unwrap();
        let w = d.release().0;
        assert_eq!(w.len(), 28);
        assert_eq!(w[0], (ADDR, std::vec![0, 0x80]));
        assert_eq!(w[14].1, [25, 0x20]); // unmuted
        assert_eq!(w[27].1, [49, 0x21]);
    }

    #[test]
    fn volume_and_mute() {
        let mut d = Es8388::new(Rec::default(), ADDR);
        d.set_volume(100).unwrap();
        d.set_volume(40).unwrap();
        d.set_mute(true).unwrap();
        assert_eq!(d.set_volume(101), Err(Error::Range));
        assert_eq!(d.set_attenuation(MAX_ATTENUATION + 1), Err(Error::Range));
        let w: Vec<Vec<u8>> = d.release().0.into_iter().map(|(_, b)| b).collect();
        assert_eq!(w, [std::vec![26, 0], std::vec![27, 0], std::vec![26, 60], std::vec![27, 60], std::vec![25, 0x24]]);
    }
}
