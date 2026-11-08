//! Chip drivers and board glue of the ESP32-P4 rev v1 family (M5Stack Tab5).
//!
//! `tab5_display_power_on` and the GT911 reader are ports of tab5-os
//! `kernel/src/platform/esp32p4v1/tab5.rs` (same author, MIT OR
//! Apache-2.0), which runs on this board; register values and the order are
//! unchanged.

use embedded_hal::i2c::I2c;
use kumiki_class::TouchPoint;
use kumiki_p4v1_hal::{
    gpio::{AnyPin, Output, Pull},
    time,
};

use super::InitError;

// PI4IOE5V6408 registers.
const EXP_ID: u8 = 0x01;
const EXP_DIR: u8 = 0x03;
const EXP_OUT: u8 = 0x05;
const EXP_OUT_H_IM: u8 = 0x07;
const EXP_PULL_EN: u8 = 0x0B;
const EXP_PULL_SEL: u8 = 0x0D;

/// The ST712x touch (ST7121 / ST7123 lots) answers here.
const ST712X: u8 = 0x55;
const ST_FW_VERSION: u16 = 0x0000;
const GT911: [u8; 2] = [0x14, 0x5D];

fn ms(n: u64) {
    time::delay_us(n * 1000);
}

/// Releases the LCD and touch resets through PI4IOE5V6408 #1 (`i2c`, at
/// 0x43; expander #2 is never touched: one of its lines cuts the power),
/// identifies the panel lot by its touch controller and synchronises the
/// GT911's INT (which also selects its address). `tp_int` is GPIO23; it is
/// left an input.
///
/// Fails with [`InitError::UnsupportedPanel`] on the ST7121 / ST7123 lots.
pub fn tab5_display_power_on<B: I2c>(i2c: &mut B, tp_int: AnyPin) -> Result<(), InitError> {
    const EXP1: u8 = 0x43;
    let mut id = [0u8];
    i2c.write_read(EXP1, &[EXP_ID], &mut id).map_err(|_| InitError::NoDevice("pi4ioe5v6408@43"))?;
    // Both resets low first, so the result is the same whatever state the
    // expander kept across a SoC reset.
    let steps = [
        (EXP_OUT, 0x44), // EXT5V_EN, CAM_RST high; LCD_RST, TP_RST, SPK_EN low
        (EXP_DIR, 0x7F), // bits 0-6 outputs, bit7 HP_DET input
        (EXP_OUT_H_IM, 0x00),
        (EXP_PULL_SEL, 0x7F),
        (EXP_PULL_EN, 0x7F),
    ];
    for (reg, v) in steps {
        i2c.write(EXP1, &[reg, v]).map_err(|_| InitError::NoDevice("pi4ioe5v6408@43"))?;
    }
    let tp = Output::new(tp_int, true);
    ms(10);
    // LCD_RST released as an input pulled up, TP_RST driven high (as
    // upstream M5GFX and esp-bsp do).
    let released = i2c.write(EXP1, &[EXP_DIR, 0x6F]).and_then(|_| i2c.write(EXP1, &[EXP_OUT, 0x64]));
    let tp = tp.into_input(Pull::None);
    released.map_err(|_| InitError::NoDevice("pi4ioe5v6408@43"))?;
    ms(100);

    // The ST712x answers with its FW version at 0x55; otherwise a GT911 ACKs
    // at 0x14 (or 0x5D). Retried every 10 ms for up to 600 ms.
    for _ in 0..60 {
        let mut fw = [0u8];
        if i2c.write_read(ST712X, &ST_FW_VERSION.to_be_bytes(), &mut fw).is_ok() {
            return Err(InitError::UnsupportedPanel);
        }
        if GT911.iter().any(|&a| i2c.write(a, &[]).is_ok()) {
            // GT911 INT synchronisation after reset (M5GFX `wakeup()`): INT
            // low for 58 ms, high for 2 ms, then input.
            let mut tp = tp.into_output(false);
            ms(58);
            embedded_hal::digital::OutputPin::set_high(&mut tp).ok();
            ms(2);
            let _ = tp.into_input(Pull::None);
            return Ok(());
        }
        ms(10);
    }
    Err(InitError::NoDevice("touch controller"))
}

const GT_STATUS: u16 = 0x814E;
const GT_POINTS: u16 = 0x814F;
const GT_POINT_SIZE: usize = 8;
const GT_READY: u8 = 1 << 7;
const GT_MAX_POINTS: usize = 5;

/// Goodix GT911 touch controller (coordinates in panel pixels).
pub struct Gt911<I> {
    /// The (shared) I²C device.
    pub i2c: I,
    /// 0x14 or 0x5D.
    pub addr: u8,
}

impl<I: I2c> kumiki_class::Touch for Gt911<I> {
    type Error = I::Error;
    fn read(&mut self, out: &mut [TouchPoint]) -> Result<usize, I::Error> {
        let mut status = [0u8];
        self.i2c.write_read(self.addr, &GT_STATUS.to_be_bytes(), &mut status)?;
        if status[0] & GT_READY == 0 {
            return Ok(0);
        }
        let count = (status[0] & 0x0F) as usize;
        let mut n = 0;
        if (1..=GT_MAX_POINTS).contains(&count) {
            // Three points (24 bytes) per transfer.
            for first in (0..count).step_by(3) {
                let k = (count - first).min(3);
                let mut b = [0u8; 3 * GT_POINT_SIZE];
                let reg = GT_POINTS + (first * GT_POINT_SIZE) as u16;
                self.i2c.write_read(self.addr, &reg.to_be_bytes(), &mut b[..k * GT_POINT_SIZE])?;
                for p in b[..k * GT_POINT_SIZE].chunks(GT_POINT_SIZE) {
                    if n < out.len() {
                        out[n] = TouchPoint {
                            id: p[0],
                            x: i16::from_le_bytes([p[1], p[2]]),
                            y: i16::from_le_bytes([p[3], p[4]]),
                        };
                        n += 1;
                    }
                }
            }
        }
        // The GT911 repeats the same report until the status is cleared.
        let [hi, lo] = GT_STATUS.to_be_bytes();
        self.i2c.write(self.addr, &[hi, lo, 0])?;
        Ok(n)
    }
}
