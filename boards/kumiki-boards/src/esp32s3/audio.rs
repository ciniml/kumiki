//! Audio of the ESP32-S3 family: one esp-hal I²S controller in full duplex
//! (RX shares TX's BCK / WS, `signal_loopback`), feeding an I²S amplifier
//! and fed by a microphone ADC, both controlled over I²C.
//!
//! * [`I2sPorts`]: the controller's two directions, built by the I²S
//!   controller node and handed to the devices on it.
//! * [`I2sOut`] / [`I2sIn`]: DMA stream buffers (esp-hal `DmaTxStreamBuf` /
//!   `DmaRxStreamBuf`). A TX stream that runs dry, or an RX stream that
//!   overflows, stops; both restart on the next use.
//! * [`Aw88298`]: Awinic AW88298 class-D amplifier (CoreS3).
//! * [`Es7210`]: Everest ES7210 4-channel microphone ADC (CoreS3).
//!
//! Register values follow M5Unified 0.2.13 (`_speaker_enabled_cb_cores3`,
//! `_microphone_enabled_cb_cores3`, MIT).

use embedded_hal::i2c::I2c;
use esp_hal::{
    Blocking,
    dma::{DmaRxStreamBuf, DmaTxStreamBuf},
    i2s::master::{I2sRx, I2sRxDmaTransfer, I2sTx, I2sTxDmaTransfer},
};
use kumiki_class::{Microphone, Speaker};

/// The two directions of one I²S controller, taken by the devices on it.
pub struct I2sPorts {
    /// Controller instance name (`"I2S0"` / `"I2S1"`).
    pub instance: &'static str,
    /// Sample rate in Hz (both directions).
    pub rate: u32,
    /// Transmit side (to an amplifier / DAC).
    pub tx: Option<I2sTx<'static, Blocking>>,
    /// Receive side (from a microphone ADC).
    pub rx: Option<I2sRx<'static, Blocking>>,
}

/// I²S input stream (16-bit stereo).
pub struct I2sIn {
    xfer: Option<I2sRxDmaTransfer<'static, Blocking, DmaRxStreamBuf>>,
    rate: u32,
    instance: &'static str,
    /// When data last arrived.
    last_data: esp_hal::time::Instant,
    /// Start of a frame cut by a descriptor that ended early (an EOF).
    carry: [u8; 4],
    carry_len: usize,
    /// Restarts after the stream overflowed (diagnostics).
    pub restarts: u32,
}

/// No data for this long means the DMA stopped (all descriptors full).
/// (The transfer's `is_done()` cannot tell: on the ESP32-S3 it reads the
/// sticky RX_DONE raw interrupt, set after the first descriptor.)
const RX_STALL_MS: u64 = 200;

/// Bytes between RX EOFs: the descriptor size of the stream buffer the
/// bindings create (`dma_rx_stream_buffer!(_, 1_000)`), a whole number of
/// 16-bit stereo frames.
pub const RX_EOF_BYTES: u32 = 1_000;

/// Fixes two settings esp-hal makes for a full-duplex stream (applied after
/// every `read` start):
///
/// * **TX_STOP_EN = 0**: esp-hal leaves it 1, which stops BCK / WS whenever
///   the TX FIFO runs empty for a moment. RX (signal loopback) and the
///   codecs run on those clocks, so the input lost frames: about 1.1 % on a
///   CoreS3 (47.46k of 48k frames per second), 48.0k with this cleared.
/// * **RX_EOF_NUM** on descriptor and frame boundaries: esp-hal writes
///   `usize::MAX - 1` into this 12-bit field, which leaves 4094 (an EOF
///   every 8190 bytes, half-way through a frame and a descriptor). Not a
///   loss by itself (`I2sIn` carries split frames over), but whole frames
///   per descriptor are simpler.
fn fix_duplex_config(instance: &str) {
    // (RX_BITS_MOD + 1) * (RX_EOF_NUM + 1) bits; 16-bit data.
    let n = (RX_EOF_BYTES * 8 / 16 - 1) as u16;
    // SAFETY: only RX_EOF_NUM is written (plus RX_UPDATE to latch it), on the
    // controller this stream owns.
    let regs = unsafe {
        match instance {
            "I2S0" => &*(esp32s3::I2S0::ptr() as *const esp32s3::i2s0::RegisterBlock),
            _ => &*(esp32s3::I2S1::ptr() as *const esp32s3::i2s0::RegisterBlock),
        }
    };
    regs.rxeof_num().write(|w| unsafe { w.rx_eof_num().bits(n) });
    regs.rx_conf().modify(|_, w| w.rx_update().set_bit());
    while regs.rx_conf().read().rx_update().bit_is_set() {}
    // Keep BCK / WS running when the TX FIFO runs empty.
    regs.tx_conf().modify(|_, w| w.tx_stop_en().clear_bit());
    regs.tx_conf().modify(|_, w| w.tx_update().set_bit());
    while regs.tx_conf().read().tx_update().bit_is_set() {}
}

impl I2sIn {
    /// Starts recording into `buf` on `rx` (controller `instance`) at `rate` Hz.
    pub fn new(rx: I2sRx<'static, Blocking>, buf: DmaRxStreamBuf, rate: u32, instance: &'static str) -> Option<Self> {
        let xfer = rx.read(buf).ok()?;
        fix_duplex_config(instance);
        Some(Self { xfer: Some(xfer), rate, instance, last_data: esp_hal::time::Instant::now(), carry: [0; 4], carry_len: 0, restarts: 0 })
    }

    /// Moves the recorded frames into `out`; returns how many.
    pub fn read(&mut self, out: &mut [[i16; 2]]) -> usize {
        if self.xfer.is_some() && self.last_data.elapsed().as_millis() > RX_STALL_MS {
            // Overflowed and stopped: restart (the frames in between are lost).
            let (rx, buf) = self.xfer.take().unwrap().stop();
            self.restarts += 1;
            self.xfer = rx.read(buf).ok();
            fix_duplex_config(self.instance);
            self.last_data = esp_hal::time::Instant::now();
            self.carry_len = 0;
        }
        let Some(x) = self.xfer.as_mut() else { return 0 };
        let mut n = 0;
        while n < out.len() {
            // Bytes of a frame split by a short descriptor come first.
            let mut b = [0u8; 260];
            let c = self.carry_len;
            b[..c].copy_from_slice(&self.carry[..c]);
            let want = ((out.len() - n) * 4).min(256);
            let got = x.pop(&mut b[c..c + want]);
            let total = c + got;
            let whole = total / 4;
            for k in 0..whole {
                let l = i16::from_le_bytes([b[k * 4], b[k * 4 + 1]]);
                let r = i16::from_le_bytes([b[k * 4 + 2], b[k * 4 + 3]]);
                out[n + k] = [l, r];
            }
            self.carry_len = total - whole * 4;
            self.carry[..self.carry_len].copy_from_slice(&b[whole * 4..total]);
            n += whole;
            if got == 0 {
                break;
            }
        }
        if n > 0 {
            self.last_data = esp_hal::time::Instant::now();
        }
        n
    }

    /// Sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }
}

/// Everest ES7210 microphone ADC fed into [`I2sIn`].
pub struct Es7210<I> {
    _i2c: I,
    input: I2sIn,
}

impl<I: I2c> Es7210<I> {
    /// Resets and configures the ADC (MIC1 / MIC2, gain 0x1B, I²S slave)
    /// with M5Unified's register sequence, then records from `input`.
    pub fn new(mut i2c: I, addr: u8, input: I2sIn) -> Result<Self, I::Error> {
        const SEQ: [(u8, u8); 31] = [
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
            (0x01, 0x14),
        ];
        for (reg, v) in SEQ {
            i2c.write(addr, &[reg, v])?;
        }
        Ok(Self { _i2c: i2c, input })
    }

    /// The I²S stream (diagnostics).
    pub fn input(&self) -> &I2sIn {
        &self.input
    }
}

impl<I: I2c> Microphone for Es7210<I> {
    type Error = I::Error;
    fn sample_rate(&self) -> u32 {
        self.input.sample_rate()
    }
    fn read(&mut self, out: &mut [[i16; 2]]) -> Result<usize, I::Error> {
        Ok(self.input.read(out))
    }
}

/// I²S output stream (16-bit stereo).
pub struct I2sOut {
    /// `Some` while streaming; the parts while (re)starting.
    xfer: Option<I2sTxDmaTransfer<'static, Blocking, DmaTxStreamBuf>>,
    rate: u32,
    /// Software volume, 0–256.
    gain: i32,
    /// Restarts after the stream ran dry (diagnostics).
    pub restarts: u32,
}

impl I2sOut {
    /// Starts streaming `buf` (zero-filled = silence) on `tx` at `rate` Hz.
    pub fn new(tx: I2sTx<'static, Blocking>, buf: DmaTxStreamBuf, rate: u32) -> Option<Self> {
        let xfer = tx.write(buf).ok()?;
        Some(Self { xfer: Some(xfer), rate, gain: 256, restarts: 0 })
    }

    fn transfer(&mut self) -> &mut I2sTxDmaTransfer<'static, Blocking, DmaTxStreamBuf> {
        if self.xfer.as_ref().is_some_and(|x| x.is_done()) {
            // Ran dry: restart the stream (what is left in the buffer plays once).
            let (tx, buf) = self.xfer.take().unwrap().stop();
            self.restarts += 1;
            self.xfer = tx.write(buf).ok();
        }
        self.xfer.as_mut().expect("I2S stream could not be restarted")
    }

    /// Pushes `frames`, waiting for room as needed.
    pub fn play(&mut self, frames: &[[i16; 2]]) {
        let gain = self.gain;
        let mut i = 0;
        while i < frames.len() {
            let x = self.transfer();
            let pushed = x.push_with(|out| {
                let n = (out.len() / 4).min(frames.len() - i);
                for (k, f) in frames[i..i + n].iter().enumerate() {
                    let l = ((f[0] as i32 * gain) >> 8) as i16;
                    let r = ((f[1] as i32 * gain) >> 8) as i16;
                    out[k * 4..k * 4 + 2].copy_from_slice(&l.to_le_bytes());
                    out[k * 4 + 2..k * 4 + 4].copy_from_slice(&r.to_le_bytes());
                }
                n * 4
            });
            i += pushed / 4;
        }
    }

    /// Tops the stream up with silence.
    pub fn idle(&mut self) {
        let x = self.transfer();
        x.push_with(|out| {
            let n = out.len() & !3;
            out[..n].fill(0);
            n
        });
    }

    /// Sample rate in Hz.
    pub fn sample_rate(&self) -> u32 {
        self.rate
    }
}

/// AW88298 register write (16-bit, big-endian).
fn aw_write<I: I2c>(i2c: &mut I, addr: u8, reg: u8, v: u16) -> Result<(), I::Error> {
    let [h, l] = v.to_be_bytes();
    i2c.write(addr, &[reg, h, l])
}

/// Awinic AW88298 amplifier fed by [`I2sOut`].
pub struct Aw88298<I> {
    i2c: I,
    addr: u8,
    out: I2sOut,
}

impl<I: I2c> Aw88298<I> {
    /// Configures the amplifier for `out`'s sample rate (16-bit stereo,
    /// BCK = 32 fs) and enables it. Its enable line (AW9523 P0.2 on CoreS3)
    /// must be high already (DT `gpio-hog`).
    pub fn new(mut i2c: I, addr: u8, out: I2sOut) -> Result<Self, I::Error> {
        // Rate code: index of the first entry ≥ rate / 2205 (rounded).
        const RATE_TBL: [u32; 10] = [4, 5, 6, 8, 10, 11, 15, 20, 22, 44];
        let r = (out.sample_rate() + 1102) / 2205;
        let code = RATE_TBL.iter().position(|&t| r <= t).unwrap_or(RATE_TBL.len() - 1) as u16;
        aw_write(&mut i2c, addr, 0x61, 0x0673)?; // boost mode disabled
        aw_write(&mut i2c, addr, 0x04, 0x4040)?; // I2SEN=1 AMPPD=0 PWDN=0
        aw_write(&mut i2c, addr, 0x05, 0x0008)?; // RMSE=0 HAGCE=0 HDCCE=0 HMUTE=0
        aw_write(&mut i2c, addr, 0x06, 0x14C0 | code)?; // I2SBCK=0 (BCK mode 16*2)
        aw_write(&mut i2c, addr, 0x0C, 0x0064)?; // volume: full
        Ok(Self { i2c, addr, out })
    }

    /// The I²S stream (diagnostics).
    pub fn output(&self) -> &I2sOut {
        &self.out
    }

    /// Powers the amplifier stage down (I2SEN=0), as M5Unified does when
    /// the speaker is disabled; [`Aw88298::new`] turns it on again.
    pub fn amp_off(&mut self) -> Result<(), I::Error> {
        aw_write(&mut self.i2c, self.addr, 0x04, 0x4000)
    }
}

impl<I: I2c> Speaker for Aw88298<I> {
    type Error = I::Error;
    fn sample_rate(&self) -> u32 {
        self.out.sample_rate()
    }
    fn play(&mut self, frames: &[[i16; 2]]) -> Result<(), I::Error> {
        self.out.play(frames);
        Ok(())
    }
    fn idle(&mut self) -> Result<(), I::Error> {
        self.out.idle();
        Ok(())
    }
    fn set_volume(&mut self, level: u8) -> Result<(), I::Error> {
        // Software gain; the amplifier stays at its full setting.
        self.out.gain = level as i32 + (level as i32 >> 7);
        Ok(())
    }
}
