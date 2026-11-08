//! Minimal on-board chip control used during bring-up.
//!
//! Register sequences follow M5GFX `src/M5GFX.cpp` and M5Unified
//! `Power_Class.cpp` (MIT, (c) M5Stack); see THIRD_PARTY_NOTICES.md.
//! These will be replaced by full driver crates bound through kumiki-dt.

use embedded_hal::i2c::I2c;
use kumiki_class::{Backlight, Power, TouchPoint};

/// AXP2101 7-bit I²C address.
pub const AXP2101_ADDR: u8 = 0x34;
/// AXP2101 chip id (register 0x03).
pub const AXP2101_ID: u8 = 0x4A;
/// AW9523B 7-bit I²C address (CoreS3).
pub const AW9523_ADDR: u8 = 0x58;
/// AW9523B id (register 0x10).
pub const AW9523_ID: u8 = 0x23;
/// FT6336U 7-bit I²C address.
pub const FT6336_ADDR: u8 = 0x38;

/// Read one 8-bit register.
pub fn read_reg<I: I2c>(i2c: &mut I, addr: u8, reg: u8) -> Result<u8, I::Error> {
    let mut b = [0];
    i2c.write_read(addr, &[reg], &mut b)?;
    Ok(b[0])
}

/// Write one 8-bit register.
pub fn write_reg<I: I2c>(i2c: &mut I, addr: u8, reg: u8, val: u8) -> Result<(), I::Error> {
    i2c.write(addr, &[reg, val])
}

// ---- AXP2101 ----------------------------------------------------------------

/// AXP2101 power-on for CoreS3: ALDO1 1.8 V (AW88298), ALDO2 3.3 V (ES7210),
/// ALDO3 3.3 V (camera), ALDO4 3.3 V (TF card), BLDO1 on.
pub fn axp2101_power_on_cores3<I: I2c>(i2c: &mut I) -> Result<(), I::Error> {
    write_reg(i2c, AXP2101_ADDR, 0x90, 0xBF)?; // LDO on/off control 0
    write_reg(i2c, AXP2101_ADDR, 0x92, 18 - 5)?;
    write_reg(i2c, AXP2101_ADDR, 0x93, 33 - 5)?;
    write_reg(i2c, AXP2101_ADDR, 0x94, 33 - 5)?;
    write_reg(i2c, AXP2101_ADDR, 0x95, 33 - 5)?;
    write_reg(i2c, AXP2101_ADDR, 0x30, 0x0F) // ADC: battery / VBUS / VSYS
}

/// LCD backlight on AXP2101 DLDO1 (CoreS3).
pub struct Axp2101Backlight<I>(pub I);

impl<I: I2c> Backlight for Axp2101Backlight<I> {
    type Error = I::Error;
    fn set_brightness(&mut self, level: u8) -> Result<(), I::Error> {
        let ctl = read_reg(&mut self.0, AXP2101_ADDR, 0x90)?;
        if level == 0 {
            return write_reg(&mut self.0, AXP2101_ADDR, 0x90, ctl & !0x80);
        }
        // M5GFX Light_M5StackCoreS3: (brightness + 641) >> 5
        write_reg(&mut self.0, AXP2101_ADDR, 0x99, ((level as u16 + 641) >> 5) as u8)?;
        write_reg(&mut self.0, AXP2101_ADDR, 0x90, ctl | 0x80)
    }
}

/// Battery / VBUS readout of the AXP2101.
pub struct Axp2101Power<I>(pub I);

impl<I: I2c> Power for Axp2101Power<I> {
    type Error = I::Error;
    fn battery_mv(&mut self) -> Result<Option<u16>, I::Error> {
        let h = read_reg(&mut self.0, AXP2101_ADDR, 0x34)? & 0x3F;
        let l = read_reg(&mut self.0, AXP2101_ADDR, 0x35)?;
        let mv = ((h as u16) << 8) | l as u16;
        Ok((mv > 0).then_some(mv))
    }
    fn vbus_present(&mut self) -> Result<Option<bool>, I::Error> {
        let st = read_reg(&mut self.0, AXP2101_ADDR, 0x00)?; // PMU status 1, bit5 = VBUS good
        Ok(Some(st & 0x20 != 0))
    }
}

// ---- AW9523B ----------------------------------------------------------------

/// AW9523B setup for CoreS3 (as in M5GFX / stackchan-rs): directions, P0
/// push-pull, outputs. `lcd_reset_released`: drive LCD_RST (P1_5) high.
pub fn aw9523_init_cores3<I: I2c>(i2c: &mut I, lcd_reset_released: bool) -> Result<(), I::Error> {
    write_reg(i2c, AW9523_ADDR, 0x04, 0b0001_1000)?; // P0 direction (1 = input)
    write_reg(i2c, AW9523_ADDR, 0x05, 0b0000_1100)?; // P1 direction
    write_reg(i2c, AW9523_ADDR, 0x11, 0b0001_0000)?; // GCR: P0 push-pull
    write_reg(i2c, AW9523_ADDR, 0x02, 0b0000_0111)?; // P0 outputs
    let p1 = if lcd_reset_released { 0b0010_0011 } else { 0b0000_0011 };
    write_reg(i2c, AW9523_ADDR, 0x03, p1)
}

// ---- M5PM1 / M5IOE1 (StopWatch) ------------------------------------------------

/// M5PM1 PMIC 7-bit address.
pub const PM1_ADDR: u8 = 0x6E;
/// M5IOE1 IO expander 7-bit address.
pub const IOE1_ADDR: u8 = 0x4F;

fn update_reg<I: I2c>(i2c: &mut I, addr: u8, reg: u8, clear: u8, set: u8) -> Result<(), I::Error> {
    let v = read_reg(i2c, addr, reg)?;
    write_reg(i2c, addr, reg, (v & !clear) | set)
}

/// StopWatch power-on: PMIC rails, then IO expander (display power, display /
/// touch reset pulse). Port of M5GFX `pmic_ops::stopwatch_power_on`
/// (MIT, (c) M5Stack).
pub fn stopwatch_power_on<I: I2c>(i2c: &mut I, delay: &mut impl embedded_hal::delay::DelayNs) -> Result<(), I::Error> {
    write_reg(i2c, PM1_ADDR, 0x09, 0x00)?;
    write_reg(i2c, PM1_ADDR, 0x0A, 0x00)?;
    update_reg(i2c, PM1_ADDR, 0x06, 0, 0x17)?;
    // The IO expander boots later than the PMIC: wait up to 500 ms for an ACK.
    let mut ready = false;
    for _ in 0..50 {
        if read_reg(i2c, IOE1_ADDR, 0x00).is_ok() {
            ready = true;
            break;
        }
        delay.delay_ms(10);
    }
    if !ready {
        // Let the next access report the error.
        read_reg(i2c, IOE1_ADDR, 0x00)?;
    }
    write_reg(i2c, IOE1_ADDR, 0x23, 0x00)?;
    update_reg(i2c, IOE1_ADDR, 0x13, 0x9D, 0)?;
    update_reg(i2c, IOE1_ADDR, 0x03, 0, 0x9D)?;
    update_reg(i2c, IOE1_ADDR, 0x05, 0, 0x99)?;
    delay.delay_ms(10);
    update_reg(i2c, IOE1_ADDR, 0x05, 0x18, 0)?; // reset pulse (display / touch)
    delay.delay_ms(8);
    update_reg(i2c, IOE1_ADDR, 0x05, 0, 0x18)?;
    delay.delay_ms(2);
    update_reg(i2c, IOE1_ADDR, 0x14, 0x02, 0)?;
    update_reg(i2c, IOE1_ADDR, 0x04, 0, 0x02)?;
    update_reg(i2c, IOE1_ADDR, 0x06, 0x02, 0)
}

// ---- CST820 ------------------------------------------------------------------

/// Hynitron CST820 touch controller (CST816-compatible register map).
pub struct Cst820<I> {
    /// The (shared) I²C device.
    pub i2c: I,
    /// Raw-to-pixel scale (StopWatch reports 0..233 for a 466 px panel).
    pub scale: i16,
}

/// CST820 7-bit address.
pub const CST820_ADDR: u8 = 0x15;

impl<I: I2c> kumiki_class::Touch for Cst820<I> {
    type Error = I::Error;
    fn read(&mut self, out: &mut [TouchPoint]) -> Result<usize, I::Error> {
        let mut b = [0u8; 5];
        self.i2c.write_read(CST820_ADDR, &[0x02], &mut b)?;
        if b[0] & 0x0F == 0 || out.is_empty() {
            return Ok(0);
        }
        let x = (((b[1] & 0x0F) as i16) << 8) | b[2] as i16;
        let y = (((b[3] & 0x0F) as i16) << 8) | b[4] as i16;
        out[0] = TouchPoint { x: x * self.scale, y: y * self.scale, id: 0 };
        Ok(1)
    }
}

// ---- FT6336U -----------------------------------------------------------------

/// FocalTech FT6336U touch controller (minimal polling reader).
pub struct Ft6336<I> {
    /// The (shared) I²C device.
    pub i2c: I,
}

impl<I: I2c> kumiki_class::Touch for Ft6336<I> {
    type Error = I::Error;
    fn read(&mut self, out: &mut [TouchPoint]) -> Result<usize, I::Error> {
        let mut b = [0u8; 13];
        self.i2c.write_read(FT6336_ADDR, &[0x02], &mut b)?;
        let n = (b[0] & 0x0F).min(2) as usize;
        let mut w = 0;
        for i in 0..n {
            if w >= out.len() {
                break;
            }
            let p = &b[1 + i * 6..];
            let x = (((p[0] & 0x0F) as i16) << 8) | p[1] as i16;
            let y = (((p[2] & 0x0F) as i16) << 8) | p[3] as i16;
            out[w] = TouchPoint { x, y, id: p[2] >> 4 };
            w += 1;
        }
        Ok(w)
    }
}
