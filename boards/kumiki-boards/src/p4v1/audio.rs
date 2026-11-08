//! Audio of the ESP32-P4 rev v1 family (Tab5): one kumiki-p4v1-hal I²S
//! controller in full duplex, shared by the DAC (speaker) and the ADC
//! (microphones) on it.
//!
//! * [`SharedI2s`]: the controller, built and started by the I²S node.
//! * [`Es8388`]: Everest ES8388 DAC driving the speaker amp.
//! * [`Es7210`]: Everest ES7210 microphone ADC.
//!
//! Register sequences are M5Unified 0.2.13's `_speaker_enabled_cb_tab5` /
//! `_microphone_enabled_cb_tab5` (MIT), verified on a Tab5 together with
//! the I²S driver (`firmware-p4/kumiki-tab5-audio`).

use core::cell::RefCell;

use embedded_hal::{digital::OutputPin, i2c::I2c};
use kumiki_class::{Microphone, Speaker};
use kumiki_p4v1_hal::i2s::I2s;

/// The I²S controller, shared by the devices on it (single core).
pub type SharedI2s = &'static RefCell<I2s>;

const ES8388_INIT: &[(u8, u8)] = &[
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
    (25, 0x20), // DAC unmute
    (26, 0x00), // LDACVOL
    (27, 0x00), // RDACVOL
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

const ES7210_INIT: &[(u8, u8)] = &[
    (0x00, 0xFF), // RESET_CTL: reset
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
    (0x43, 0x1B), // MIC1_GAIN
    (0x44, 0x1B), // MIC2_GAIN
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

/// The controller's sample rate in Hz (rounded).
fn rate(i2s: SharedI2s) -> u32 {
    ((i2s.borrow().actual_sample_rate_mhz() + 500) / 1000) as u32
}

fn write_all<I: I2c>(i2c: &mut I, addr: u8, regs: &[(u8, u8)]) -> Result<(), I::Error> {
    for &(r, v) in regs {
        i2c.write(addr, &[r, v])?;
    }
    Ok(())
}

/// Everest ES8388 DAC (I²S slave) with the speaker amplifier behind it.
pub struct Es8388<I> {
    _i2c: I,
    i2s: SharedI2s,
    /// Software gain, 0–256 (the DAC stays at 0 dB).
    gain: i32,
}

impl<I: I2c> Es8388<I> {
    /// Configures the DAC (the I²S controller must be running: the codec
    /// needs MCLK), then turns the amplifier on through `amp`.
    pub fn new(mut i2c: I, addr: u8, i2s: SharedI2s, amp: &mut impl OutputPin) -> Result<Self, I::Error> {
        write_all(&mut i2c, addr, ES8388_INIT)?;
        let _ = amp.set_high();
        Ok(Self { _i2c: i2c, i2s, gain: 256 })
    }
}

impl<I: I2c> Speaker for Es8388<I> {
    type Error = I::Error;
    fn sample_rate(&self) -> u32 {
        rate(self.i2s)
    }
    fn play(&mut self, frames: &[[i16; 2]]) -> Result<(), I::Error> {
        // Scaled in chunks (same linear volume as the ESP32-S3 family).
        let mut chunk = [[0i16; 2]; 64];
        for part in frames.chunks(chunk.len()) {
            for (o, f) in chunk.iter_mut().zip(part) {
                *o = [((f[0] as i32 * self.gain) >> 8) as i16, ((f[1] as i32 * self.gain) >> 8) as i16];
            }
            let mut i = 0;
            while i < part.len() {
                i += self.i2s.borrow_mut().push_frames(&chunk[i..part.len()]);
            }
        }
        Ok(())
    }
    fn idle(&mut self) -> Result<(), I::Error> {
        // The driver turns played descriptors into silence while polled.
        self.i2s.borrow_mut().poll();
        Ok(())
    }
    fn set_volume(&mut self, level: u8) -> Result<(), I::Error> {
        // Linear amplitude like the ESP32-S3 family (level / 255).
        self.gain = level as i32 + (level as i32 >> 7);
        Ok(())
    }
}

/// Everest ES7210 microphone ADC (MIC1 / MIC2) on the shared controller.
pub struct Es7210<I> {
    _i2c: I,
    i2s: SharedI2s,
}

impl<I: I2c> Es7210<I> {
    /// Resets and configures the ADC (MCLK must be running).
    pub fn new(mut i2c: I, addr: u8, i2s: SharedI2s) -> Result<Self, I::Error> {
        write_all(&mut i2c, addr, ES7210_INIT)?;
        Ok(Self { _i2c: i2c, i2s })
    }
}

impl<I: I2c> Microphone for Es7210<I> {
    type Error = I::Error;
    fn sample_rate(&self) -> u32 {
        rate(self.i2s)
    }
    fn read(&mut self, out: &mut [[i16; 2]]) -> Result<usize, I::Error> {
        Ok(self.i2s.borrow_mut().pop_frames(out))
    }
}
