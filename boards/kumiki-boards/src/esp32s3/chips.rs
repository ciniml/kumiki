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

/// AXP2101 LDO channels (DT regulator node names).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AxpLdo {
    /// ALDO1..ALDO4
    Aldo1,
    #[allow(missing_docs)]
    Aldo2,
    #[allow(missing_docs)]
    Aldo3,
    #[allow(missing_docs)]
    Aldo4,
    /// BLDO1, BLDO2
    Bldo1,
    #[allow(missing_docs)]
    Bldo2,
    /// DLDO1 (CoreS3: LCD backlight)
    Dldo1,
}

impl AxpLdo {
    /// (enable bit in reg 0x90, voltage register)
    const fn regs(self) -> (u8, u8) {
        match self {
            AxpLdo::Aldo1 => (0, 0x92),
            AxpLdo::Aldo2 => (1, 0x93),
            AxpLdo::Aldo3 => (2, 0x94),
            AxpLdo::Aldo4 => (3, 0x95),
            AxpLdo::Bldo1 => (4, 0x96),
            AxpLdo::Bldo2 => (5, 0x97),
            AxpLdo::Dldo1 => (7, 0x99),
        }
    }
}

/// Apply DT regulator settings to an AXP2101: each entry `(ldo, millivolts)`
/// is enabled, and its voltage set when `millivolts` is non-zero
/// (0.5–3.5 V in 100 mV steps). Also enables the battery / VBUS / VSYS ADC.
pub fn axp2101_init<I: I2c>(i2c: &mut I, addr: u8, regulators: &[(AxpLdo, u16)]) -> Result<(), I::Error> {
    let mut enable = read_reg(i2c, addr, 0x90)?;
    for &(ldo, mv) in regulators {
        let (bit, vreg) = ldo.regs();
        if mv != 0 {
            write_reg(i2c, addr, vreg, (mv.clamp(500, 3500) / 100 - 5) as u8)?;
        }
        enable |= 1 << bit;
    }
    write_reg(i2c, addr, 0x90, enable)?;
    write_reg(i2c, addr, 0x30, 0x0F) // ADC: battery / VBUS / VSYS
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

/// Initial state of an IO-expander line (from DT `gpio-hog` nodes and from
/// the lines other devices reference, e.g. a panel's `reset-gpios`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineInit {
    /// Input.
    Input,
    /// Output, driven high.
    OutputHigh,
    /// Output, driven low.
    OutputLow,
}

/// Configure an AW9523B: lines 0..7 = P0_0..P0_7, 8..15 = P1_0..P1_7.
/// Unlisted lines become inputs. `p0_push_pull`: GCR bit 4.
pub fn aw9523_init<I: I2c>(i2c: &mut I, addr: u8, p0_push_pull: bool, lines: &[(u8, LineInit)]) -> Result<(), I::Error> {
    let (mut dir, mut out) = (0xFFFFu16, 0u16); // 1 = input
    for &(line, init) in lines {
        let m = 1u16 << line;
        match init {
            LineInit::Input => {}
            LineInit::OutputHigh => {
                dir &= !m;
                out |= m;
            }
            LineInit::OutputLow => dir &= !m,
        }
    }
    write_reg(i2c, addr, 0x04, dir as u8)?;
    write_reg(i2c, addr, 0x05, (dir >> 8) as u8)?;
    write_reg(i2c, addr, 0x11, if p0_push_pull { 0x10 } else { 0x00 })?;
    write_reg(i2c, addr, 0x02, out as u8)?;
    write_reg(i2c, addr, 0x03, (out >> 8) as u8)
}

/// Configure a PY32 IO expander's listed lines (the others keep their
/// state) and its LED count (`leds`, DT `kumiki,leds`; the LEDs start off).
pub fn py32_init<I: I2c>(i2c: I, addr: u8, leds: Option<u32>, lines: &[(u8, LineInit)]) -> Result<kumiki_py32::Py32<I>, I::Error> {
    use kumiki_py32::{Error, LineMode, MAX_LEDS, Py32};
    // Only I²C errors propagate (an out-of-range line is skipped).
    fn ok<E>(r: Result<(), Error<E>>) -> Result<(), E> {
        match r {
            Err(Error::I2c(e)) => Err(e),
            _ => Ok(()),
        }
    }
    let mut d = Py32::new(i2c, addr);
    for &(line, init) in lines {
        let mode = match init {
            LineInit::Input => LineMode::Input,
            LineInit::OutputHigh => LineMode::Output(true),
            LineInit::OutputLow => LineMode::Output(false),
        };
        ok(d.set_line_mode(line, mode))?;
    }
    let n = leds.unwrap_or(0).min(MAX_LEDS as u32) as usize;
    if n > 0 {
        ok(d.set_led_count(n as u8))?;
        ok(d.set_leds(0, &[0; MAX_LEDS as usize][..n]))?;
        ok(d.show_leds())?;
    }
    Ok(d)
}

/// One AW9523B output line as an `embedded_hal::digital::OutputPin`
/// (e.g. a panel reset line on the expander).
pub struct Aw9523Pin<I> {
    /// The (shared) I²C device.
    pub i2c: I,
    /// 7-bit address.
    pub addr: u8,
    /// Line 0..15.
    pub line: u8,
}

/// Error of an expander pin.
#[derive(Debug)]
pub struct ExpanderPinError;

impl embedded_hal::digital::Error for ExpanderPinError {
    fn kind(&self) -> embedded_hal::digital::ErrorKind {
        embedded_hal::digital::ErrorKind::Other
    }
}

impl<I: I2c> embedded_hal::digital::ErrorType for Aw9523Pin<I> {
    type Error = ExpanderPinError;
}

impl<I: I2c> embedded_hal::digital::OutputPin for Aw9523Pin<I> {
    fn set_low(&mut self) -> Result<(), ExpanderPinError> {
        let reg = 0x02 + self.line / 8;
        let v = read_reg(&mut self.i2c, self.addr, reg).map_err(|_| ExpanderPinError)?;
        write_reg(&mut self.i2c, self.addr, reg, v & !(1 << (self.line % 8))).map_err(|_| ExpanderPinError)
    }
    fn set_high(&mut self) -> Result<(), ExpanderPinError> {
        let reg = 0x02 + self.line / 8;
        let v = read_reg(&mut self.i2c, self.addr, reg).map_err(|_| ExpanderPinError)?;
        write_reg(&mut self.i2c, self.addr, reg, v | (1 << (self.line % 8))).map_err(|_| ExpanderPinError)
    }
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
    /// Raw → pixel mapping.
    pub map: TouchMap,
    /// Lazily applied configuration done (see `ensure_init`).
    pub inited: bool,
}

/// Linear raw → pixel mapping per axis, in the spirit of the Linux / DT
/// touchscreen properties: raw `min..min+size` spans pixels `0..pixels`.
#[derive(Clone, Copy, Debug)]
pub struct TouchMap {
    /// `touchscreen-min-x` / `-y`: raw value at pixel 0.
    pub min: (i32, i32),
    /// `touchscreen-size-x` / `-y`: raw span of the whole panel.
    pub size: (i32, i32),
    /// Panel size in pixels.
    pub pixels: (i32, i32),
}

impl TouchMap {
    /// Identity mapping for a `w × h` panel.
    pub const fn identity(w: i32, h: i32) -> Self {
        Self { min: (0, 0), size: (w, h), pixels: (w, h) }
    }
    /// Map a raw point to pixels (clamped to the panel).
    pub fn apply(&self, x: i32, y: i32) -> (i16, i16) {
        let px = ((x - self.min.0) * self.pixels.0 / self.size.0).clamp(0, self.pixels.0 - 1);
        let py = ((y - self.min.1) * self.pixels.1 / self.size.1).clamp(0, self.pixels.1 - 1);
        (px as i16, py as i16)
    }
}

impl<I: I2c> Cst820<I> {
    /// Like M5GFX `Touch_CST816S::_check_init`: the chip may NAK while in
    /// low-power mode, so configure it on the first successful access.
    fn ensure_init(&mut self) {
        if self.inited {
            return;
        }
        let mut id = [0u8; 3];
        if self.i2c.write(CST820_ADDR, &[0x00, 0x00]).is_ok() && self.i2c.write_read(CST820_ADDR, &[0xA7], &mut id).is_ok() {
            let _ = self.i2c.write(CST820_ADDR, &[0xFA, 0x20]); // INT pulse on change
            let _ = self.i2c.write(CST820_ADDR, &[0xED, 20]); // INT pulse width 2 ms
            self.inited = true;
        }
    }
}

/// CST820 7-bit address.
pub const CST820_ADDR: u8 = 0x15;

impl<I: I2c> kumiki_class::Touch for Cst820<I> {
    type Error = I::Error;
    fn read(&mut self, out: &mut [TouchPoint]) -> Result<usize, I::Error> {
        self.ensure_init();
        let mut b = [0u8; 5];
        self.i2c.write_read(CST820_ADDR, &[0x02], &mut b)?;
        if b[0] & 0x0F == 0 || out.is_empty() {
            return Ok(0);
        }
        let x = (((b[1] & 0x0F) as i32) << 8) | b[2] as i32;
        let y = (((b[3] & 0x0F) as i32) << 8) | b[4] as i32;
        let (x, y) = self.map.apply(x, y);
        out[0] = TouchPoint { x, y, id: 0 };
        Ok(1)
    }
}

// ---- CST9217 -----------------------------------------------------------------

/// CST9217 7-bit address (two-finger variant fitted to some StopWatch units).
pub const CST9217_ADDR: u8 = 0x5A;

/// Hynitron CST9217 touch controller. Port of M5GFX `Touch_CST9217::getTouchRaw`
/// (BSD-2-Clause, LovyanGFX / MIT, M5Stack).
pub struct Cst9217<I> {
    /// The (shared) I²C device.
    pub i2c: I,
}

impl<I: I2c> kumiki_class::Touch for Cst9217<I> {
    type Error = I::Error;
    fn read(&mut self, out: &mut [TouchPoint]) -> Result<usize, I::Error> {
        let mut d = [0u8; 15];
        let r = self.i2c.write_read(CST9217_ADDR, &[0xD0, 0x00], &mut d);
        let _ = self.i2c.write(CST9217_ADDR, &[0xD0, 0x00, 0xAB]); // acknowledge the report
        r?;
        if d[6] != 0xAB || d[0] == 0xAB || d[0] == 0 {
            return Ok(0);
        }
        let slots = ((d[5] & 0x7F) as usize).min(2);
        let mut n = 0;
        for i in 0..slots {
            if n >= out.len() {
                break;
            }
            let o = i * 5 + if i > 0 { 2 } else { 0 };
            if d[o] & 0x0F != 0x06 {
                continue; // not "pressed"
            }
            let x = ((d[o + 1] as i16) << 4) | (d[o + 3] >> 4) as i16;
            let y = ((d[o + 2] as i16) << 4) | (d[o + 3] & 0x0F) as i16;
            out[n] = TouchPoint { x, y, id: d[o] >> 4 };
            n += 1;
        }
        Ok(n)
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
