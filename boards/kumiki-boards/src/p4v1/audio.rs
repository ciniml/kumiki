//! Audio of the ESP32-P4 rev v1 family (Tab5): one kumiki-p4v1-hal I²S
//! controller in full duplex, shared by the DAC (speaker) and the ADC
//! (microphones) on it.
//!
//! * [`SharedI2s`]: the controller, built and started by the I²S node.
//! * [`Es8388`]: Everest ES8388 DAC ([`kumiki_es8388`]) with the speaker
//!   amplifier, whose enable line is the board's (an `OutputPin`).
//! * [`Es7210`]: Everest ES7210 microphone ADC ([`kumiki_es7210`]).

use core::cell::RefCell;

use embedded_hal::{digital::OutputPin, i2c::I2c};
use kumiki_class::{Microphone, Speaker};
use kumiki_p4v1_hal::i2s::I2s;

/// The I²S controller, shared by the devices on it (single core).
pub type SharedI2s = &'static RefCell<I2s>;

/// The controller's sample rate in Hz (rounded).
fn rate(i2s: SharedI2s) -> u32 {
    ((i2s.borrow().actual_sample_rate_mhz() + 500) / 1000) as u32
}

/// Everest ES8388 DAC (I²S slave) with the speaker amplifier behind it.
pub struct Es8388<I> {
    codec: kumiki_es8388::Es8388<I>,
    i2s: SharedI2s,
    /// Software gain, 0–256 (the DAC stays at 0 dB).
    gain: i32,
}

impl<I: I2c> Es8388<I> {
    /// Configures the DAC (the I²S controller must be running: the codec
    /// needs MCLK), then turns the amplifier on through `amp`.
    pub fn new(i2c: I, addr: u8, i2s: SharedI2s, amp: &mut impl OutputPin) -> Result<Self, kumiki_es8388::Error<I::Error>> {
        let mut codec = kumiki_es8388::Es8388::new(i2c, addr);
        codec.init()?;
        let _ = amp.set_high();
        Ok(Self { codec, i2s, gain: 256 })
    }

    /// The codec (DAC attenuation, mute).
    pub fn codec(&mut self) -> &mut kumiki_es8388::Es8388<I> {
        &mut self.codec
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
    codec: kumiki_es7210::Es7210<I>,
    i2s: SharedI2s,
}

impl<I: I2c> Es7210<I> {
    /// Resets and configures the ADC (MCLK must be running).
    pub fn new(i2c: I, addr: u8, i2s: SharedI2s) -> Result<Self, kumiki_es7210::Error<I::Error>> {
        let mut codec = kumiki_es7210::Es7210::new(i2c, addr);
        codec.init()?;
        Ok(Self { codec, i2s })
    }

    /// The codec (gain).
    pub fn codec(&mut self) -> &mut kumiki_es7210::Es7210<I> {
        &mut self.codec
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
