//! I2S0 full-duplex test on the M5Stack Tab5 (ESP32-P4 rev v1).
//!
//! Brings up I2S0 (MCLK GPIO30, BCK GPIO27, WS GPIO29, DOUT GPIO26 to the
//! ES8388, DIN GPIO28 from the ES7210) at 48 kHz, initialises the ES8388
//! DAC and the ES7210 mic ADC over the internal I²C (I2C1, SDA GPIO31,
//! SCL GPIO32) with M5Unified 0.2.13's sequences, and turns the NS4150B
//! speaker amp on (PI4IOE5V6408 #1 at 0x43, bit 1; expander #2 at 0x44 is
//! never touched: one of its lines cuts the power).
//!
//! After 1 s of silence it plays a 1 kHz sine for 2 s (plus the ring
//! pre-fill), then silence. Once per second it prints the frames received
//! per second and the peak / RMS of both mic channels; after the tone, the
//! measured TX rate (pushing 2 s of samples once the ring is full).

#![no_std]
#![no_main]

use embedded_hal::i2c::I2c as _;
use kumiki_p4v1_hal::{
    self as hal,
    clock,
    i2c::I2c,
    i2s::{self, I2s},
    println, time,
};

hal::entry!(main);

const ES8388: u8 = 0x10;
const ES7210: u8 = 0x40;
const EXP1: u8 = 0x43;

/// M5Unified `_speaker_enabled_cb_tab5` (M5Unified.cpp:497-529), verbatim.
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

/// M5Unified `_microphone_enabled_cb_tab5` (M5Unified.cpp:885-925),
/// verbatim (after the initial write of 0xFF to register 0x00).
const ES7210_INIT: &[(u8, u8)] = &[
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

/// Sine amplitude (about -18 dBFS).
const AMP: f32 = 4096.0;
/// 1 kHz at 48 kHz: one period is 48 frames.
const PERIOD: usize = 48;
const FS: u32 = 48_000;
/// Tone length after the ring pre-fill.
const TONE_FRAMES: u64 = 2 * FS as u64;

fn bulk_write(i2c: &mut I2c, addr: u8, name: &str, regs: &[(u8, u8)]) {
    let mut failed = 0;
    for &(r, v) in regs {
        if let Err(e) = i2c.write(addr, &[r, v]) {
            println!("{name}: write {r:#04x}={v:#04x} failed: {e:?}");
            failed += 1;
        }
    }
    println!("{name}@{addr:#04x}: {} registers written, {failed} failed", regs.len());
}

fn read_reg(i2c: &mut I2c, addr: u8, reg: u8) -> Option<u8> {
    let mut v = [0u8];
    i2c.write_read(addr, &[reg], &mut v).ok().map(|_| v[0])
}

/// Turns the speaker amp on: bit 1 of PI4IOE5V6408 #1, and only that bit
/// (read-modify-write). The pin is also made an output (DIR bit 1) and
/// not high-impedance (OUT_H_IM bit 1) in case nothing has set the
/// expander up since power-on.
fn amp_on(i2c: &mut I2c) {
    const DIR: u8 = 0x03;
    const OUT: u8 = 0x05;
    const OUT_H_IM: u8 = 0x07;
    const SPK_EN: u8 = 1 << 1;
    let (Some(dir), Some(out), Some(him)) = (read_reg(i2c, EXP1, DIR), read_reg(i2c, EXP1, OUT), read_reg(i2c, EXP1, OUT_H_IM)) else {
        println!("pi4ioe@0x43: read failed, amp left alone");
        return;
    };
    println!("pi4ioe@0x43 before: dir {dir:#04x} out {out:#04x} out_h_im {him:#04x}");
    let mut ok = true;
    if dir & SPK_EN == 0 {
        ok &= i2c.write(EXP1, &[DIR, dir | SPK_EN]).is_ok();
    }
    if him & SPK_EN != 0 {
        ok &= i2c.write(EXP1, &[OUT_H_IM, him & !SPK_EN]).is_ok();
    }
    ok &= i2c.write(EXP1, &[OUT, out | SPK_EN]).is_ok();
    let after = (read_reg(i2c, EXP1, DIR), read_reg(i2c, EXP1, OUT), read_reg(i2c, EXP1, OUT_H_IM));
    println!("pi4ioe@0x43 after: {after:02x?} (writes ok: {ok})");
}

fn isqrt(v: u64) -> u64 {
    if v < 2 {
        return v;
    }
    let mut x = 1u64 << ((64 - v.leading_zeros()).div_ceil(2));
    loop {
        let y = (x + v / x) / 2;
        if y >= x {
            return x;
        }
        x = y;
    }
}

/// Per-second mic statistics.
#[derive(Default)]
struct Levels {
    frames: u64,
    peak: [u16; 2],
    sum_sq: [u64; 2],
}

impl Levels {
    fn add(&mut self, f: &[[i16; 2]]) {
        for s in f {
            for c in 0..2 {
                let v = s[c] as i32;
                self.peak[c] = self.peak[c].max(v.unsigned_abs().min(u16::MAX as u32) as u16);
                self.sum_sq[c] += (v * v) as u64;
            }
        }
        self.frames += f.len() as u64;
    }
    fn rms(&self, c: usize) -> u64 {
        if self.frames == 0 { 0 } else { isqrt(self.sum_sq[c] / self.frames) }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Phase {
    Baseline,
    Prefill,
    Tone,
    Silence,
}

fn main() -> ! {
    println!();
    println!("kumiki-tab5-audio (kumiki-p4v1-hal I2S0 full duplex)");
    match clock::init_max() {
        Ok(mhz) => println!("cpu {mhz} MHz"),
        Err(e) => println!("clock: {e}"),
    }

    let p = hal::take().unwrap();
    let mut i2c = I2c::new(p.i2c1, p.pins.gpio31, p.pins.gpio32, 100_000);
    for (name, addr) in [("es8388", ES8388), ("es7210", ES7210), ("pi4ioe#1", EXP1)] {
        let ack = i2c.write(addr, &[]).is_ok();
        println!("i2c1 {name}@{addr:#04x}: {}", if ack { "ACK" } else { "no ACK" });
    }

    let pins = i2s::Pins {
        mclk: p.pins.gpio30.into(),
        bclk: p.pins.gpio27.into(),
        ws: p.pins.gpio29.into(),
        dout: p.pins.gpio26.into(),
        din: p.pins.gpio28.into(),
    };
    let mut i2s = match I2s::new(p.i2s0, pins, i2s::Config { sample_rate: FS, ..Default::default() }) {
        Ok(i) => i,
        Err(e) => panic!("i2s: {e:?}"),
    };
    let d = i2s.divider();
    let fs_mhz = i2s.actual_sample_rate_mhz();
    println!(
        "i2s: mclk div {} {}/{}, fs {}.{:03} Hz, tx ring {} us",
        d.integer,
        d.numerator,
        d.denominator,
        fs_mhz / 1000,
        fs_mhz % 1000,
        i2s.tx_ring_us()
    );
    i2s.start();
    time::delay_us(10_000);

    bulk_write(&mut i2c, ES8388, "es8388", ES8388_INIT);
    amp_on(&mut i2c);
    if let Err(e) = i2c.write(ES7210, &[0x00, 0xFF]) {
        println!("es7210: reset write failed: {e:?}");
    }
    bulk_write(&mut i2c, ES7210, "es7210", ES7210_INIT);

    // One period of the sine, by rotating a unit vector 7.5 degrees a step.
    let (c, s) = (0.991_444_86_f32, 0.130_526_19_f32);
    let mut sine = [0i16; PERIOD];
    let (mut x, mut y) = (1.0f32, 0.0f32);
    for v in sine.iter_mut() {
        *v = (y * AMP) as i16;
        (x, y) = (x * c - y * s, x * s + y * c);
    }

    let t_boot = time::now_us();
    let mut phase = Phase::Baseline;
    let mut sine_pos = 0usize;
    let mut tone_frames = 0u64;
    let mut prefill_frames = 0u64;
    let mut t_tone0 = 0u64;
    let mut tx_rate_report: Option<(u64, u64)> = None;
    let mut chunk = [[0i16; 2]; 128];
    let mut rx = [[0i16; 2]; 256];
    let mut lv = Levels::default();
    let mut t_report = t_boot;
    let mut last_tx_descs = 0u64;
    let mut printed_raw = false;
    let mut second = 0u32;
    let (mut det_i, mut det_q, mut det_n, mut det_n0) = (0f32, 0f32, 0u64, 0u64);

    loop {
        let now = time::now_us();
        // TX.
        match phase {
            Phase::Baseline if now - t_boot >= 1_000_000 => {
                phase = Phase::Prefill;
                println!("--- tone on (1 kHz, amplitude {AMP})");
            }
            Phase::Prefill | Phase::Tone => {
                let want = if phase == Phase::Tone { (TONE_FRAMES - tone_frames).min(chunk.len() as u64) as usize } else { chunk.len() };
                for (k, f) in chunk[..want].iter_mut().enumerate() {
                    let v = sine[(sine_pos + k) % PERIOD];
                    *f = [v, v];
                }
                let n = i2s.push_frames(&chunk[..want]);
                sine_pos = (sine_pos + n) % PERIOD;
                if phase == Phase::Prefill {
                    prefill_frames += n as u64;
                    if n < want {
                        // The ring is full: time the next 2 s of samples.
                        phase = Phase::Tone;
                        t_tone0 = time::now_us();
                    }
                } else {
                    tone_frames += n as u64;
                    if tone_frames >= TONE_FRAMES {
                        let dt = time::now_us() - t_tone0;
                        tx_rate_report = Some((tone_frames, dt));
                        phase = Phase::Silence;
                        println!("--- tone off (pushed {prefill_frames} pre-fill + {tone_frames} frames)");
                    }
                }
            }
            _ => {}
        }
        // RX.
        loop {
            let n = i2s.pop_frames(&mut rx);
            if n == 0 {
                break;
            }
            if !printed_raw && now - t_boot > 500_000 {
                printed_raw = true;
                println!("rx raw: {:?}", &rx[..n.min(4)]);
            }
            lv.add(&rx[..n]);
            // 1 kHz detector (left channel): correlate with one sine / cosine period.
            for f in &rx[..n] {
                let k = (det_n % PERIOD as u64) as usize;
                let x = f[0] as f32;
                det_i += x * sine[k] as f32;
                det_q += x * sine[(k + PERIOD / 4) % PERIOD] as f32;
                det_n += 1;
            }
        }
        // Report.
        if now - t_report >= 1_000_000 {
            let dt = now - t_report;
            t_report = now;
            second += 1;
            let st = i2s.stats();
            let tx_fps = (st.tx_descs_done - last_tx_descs) * (i2s::DESC_LEN / i2s::FRAME_BYTES) as u64 * 1_000_000 / dt;
            last_tx_descs = st.tx_descs_done;
            println!(
                "[{second:3} s {:8}] rx {:5} fr/s  L peak {:5} rms {:5}  R peak {:5} rms {:5} | dma tx {tx_fps} fr/s, underruns {}, rx overruns {}, short {} (last {}), int tx {:#x} rx {:#x}",
                match phase {
                    Phase::Baseline => "silence",
                    Phase::Prefill | Phase::Tone => "tone",
                    Phase::Silence => "silence",
                },
                lv.frames * 1_000_000 / dt,
                lv.peak[0],
                lv.rms(0),
                lv.peak[1],
                lv.rms(1),
                st.tx_underruns,
                st.rx_overruns,
                st.rx_short_descs,
                st.rx_last_short_len,
                st.tx_dma_int_raw,
                st.rx_dma_int_raw,
            );
            // Amplitude of the 1 kHz component in the mic signal (LSB).
            let frames = (det_n - det_n0).max(1) as f32;
            let mag = isqrt(((det_i * det_i + det_q * det_q) / (frames * frames)) as u64) as f32 * 2.0 / AMP;
            println!("    1 kHz in mic: {mag:.1} LSB");
            (det_i, det_q, det_n0) = (0.0, 0.0, det_n);
            lv = Levels::default();
            if let Some((frames, us)) = tx_rate_report.take() {
                println!("tx rate: {frames} frames pushed in {us} us after the ring was full = {} fr/s", frames * 1_000_000 / us);
            }
        }
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    println!("\n*** panic: {info}");
    loop {
        core::hint::spin_loop();
    }
}
