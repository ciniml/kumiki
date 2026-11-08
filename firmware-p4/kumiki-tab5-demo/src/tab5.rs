//! Tab5 board bring-up for the display, ported from tab5-os
//! (`kernel/src/platform/esp32p4v1/tab5.rs`, doc/17 §5,
//! doc/research/m6-panel-touch.md §2, §3, §6): IO expander #1 (0x43)
//! releases the LCD and touch resets, then the touch controller on I²C
//! tells which panel this is.
//!
//! SAFETY of the board: IO expander #2 (0x44) is only read here. Its bit4
//! powers the Tab5 off (tab5-os doc/06 §1.3), so nothing ever writes it.

use embedded_hal::i2c::I2c;
use kumiki_p4v1_hal::gpio::{AnyPin, Output, Pull};
use kumiki_p4v1_hal::{println, time};

// PI4IOE5V6408 registers (datasheet table 2; m6-panel-touch.md §2.1).
const EXP1: u8 = 0x43;
const EXP2: u8 = 0x44;
const EXP_ID: u8 = 0x01;
const EXP_DIR: u8 = 0x03;
const EXP_OUT: u8 = 0x05;
const EXP_OUT_H_IM: u8 = 0x07;
const EXP_PULL_EN: u8 = 0x0B;
const EXP_PULL_SEL: u8 = 0x0D;
const EXP_IN: u8 = 0x0F;

// Touch controllers.
const ST712X: u8 = 0x55;
const GT911: [u8; 2] = [0x14, 0x5D];
const ST_FW_VERSION: u16 = 0x0000;
const ST_MAX_TOUCHES: u16 = 0x0009;

/// The panel lot, told by its touch controller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    /// The IO expander did not answer.
    None,
    /// Nothing identified on the touch addresses.
    Unknown,
    /// ILI9881C display with a GT911 touch controller at `touch_addr`.
    Ili9881c { touch_addr: u8 },
    /// ST7123 display and touch.
    St7123,
    /// ST7121 display and touch.
    St7121,
}

fn ms(n: u64) {
    time::delay_us(n * 1000);
}

fn exp_write<B: I2c>(i2c: &mut B, addr: u8, reg: u8, v: u8) -> Result<(), B::Error> {
    i2c.write(addr, &[reg, v])
}

fn exp_read<B: I2c>(i2c: &mut B, addr: u8, reg: u8) -> Result<u8, B::Error> {
    let mut b = [0u8];
    i2c.write_read(addr, &[reg], &mut b).map(|_| b[0])
}

fn reg16_read<B: I2c>(i2c: &mut B, addr: u8, reg: u16, out: &mut [u8]) -> Result<(), B::Error> {
    i2c.write_read(addr, &reg.to_be_bytes(), out)
}

fn show<E: core::fmt::Debug>(r: &Result<u8, E>) -> i32 {
    r.as_ref().map_or(-1, |&v| v as i32)
}

/// Releases the LCD and touch resets and identifies the panel. `tp_int` is
/// GPIO23 (the touch INT, which also selects the GT911's address); it is
/// left an input.
pub fn init<B: I2c>(i2c: &mut B, tp_int: AnyPin) -> Panel {
    let id1 = exp_read(i2c, EXP1, EXP_ID);
    let id2 = exp_read(i2c, EXP2, EXP_ID);
    // #2's inputs: bit6 CHG_STAT (charging), read only.
    let in2 = exp_read(i2c, EXP2, EXP_IN);
    println!("board: expanders exp1_id {} exp2_id {} exp2_in {}", show(&id1), show(&id2), show(&in2));
    if let Err(e) = id1 {
        println!("board: no expander ({e:?})");
        return Panel::None;
    }
    // Both resets low first, so the result is the same whatever state the
    // expander kept across a SoC reset (m6-panel-touch.md §2.3, §7).
    let steps = [
        (EXP_OUT, 0x44), // EXT5V_EN, CAM_RST high; LCD_RST, TP_RST, SPK_EN low
        (EXP_DIR, 0x7F), // bits 0-6 outputs, bit7 HP_DET input
        (EXP_OUT_H_IM, 0x00),
        (EXP_PULL_SEL, 0x7F),
        (EXP_PULL_EN, 0x7F),
    ];
    for (reg, v) in steps {
        if let Err(e) = exp_write(i2c, EXP1, reg, v) {
            println!("board: expander write {reg:#04x} failed ({e:?})");
            return Panel::None;
        }
    }
    let tp = Output::new(tp_int, true);
    ms(10);
    // LCD_RST released as an input pulled up, TP_RST driven high (as
    // upstream M5GFX and esp-bsp do).
    let released = exp_write(i2c, EXP1, EXP_DIR, 0x6F).and_then(|_| exp_write(i2c, EXP1, EXP_OUT, 0x64));
    let tp = tp.into_input(Pull::None);
    if let Err(e) = released {
        println!("board: expander write {EXP_DIR:#04x} failed ({e:?})");
        return Panel::None;
    }
    ms(100);

    let panel = identify(i2c);
    if let Panel::Ili9881c { .. } = panel {
        // GT911's INT synchronisation after reset, as M5GFX's `wakeup()`:
        // INT low for 58 ms, high for 2 ms, then input.
        let mut tp = tp.into_output(false);
        ms(58);
        embedded_hal::digital::OutputPin::set_high(&mut tp).ok();
        ms(2);
        let _ = tp.into_input(Pull::None);
    }
    panel
}

/// m6-panel-touch.md §3.2: the ST712x touch answers at 0x55 with its FW
/// version (1 = ST7121, 3 = ST7123); otherwise a GT911 ACKs at 0x14 (or
/// 0x5D). Retried every 10 ms for up to 600 ms: the ST712x needs tens of
/// ms after a reset.
fn identify<B: I2c>(i2c: &mut B) -> Panel {
    for attempt in 0..60u32 {
        let mut fw = [0u8];
        match reg16_read(i2c, ST712X, ST_FW_VERSION, &mut fw) {
            Ok(()) => {
                let mut max = [0u8];
                let _ = reg16_read(i2c, ST712X, ST_MAX_TOUCHES, &mut max);
                println!("board: panel st712x fw {} touch {ST712X:#04x} attempts {} max_touches {}", fw[0], attempt + 1, max[0]);
                return match fw[0] {
                    1 => Panel::St7121,
                    3 => Panel::St7123,
                    _ => Panel::Unknown,
                };
            }
            Err(_) => {
                if let Some(&a) = GT911.iter().find(|&&a| i2c.write(a, &[]).is_ok()) {
                    println!("board: panel ili9881c, touch gt911 at {a:#04x}, attempts {}", attempt + 1);
                    return Panel::Ili9881c { touch_addr: a };
                }
            }
        }
        ms(10);
    }
    println!("board: panel unknown");
    Panel::Unknown
}
