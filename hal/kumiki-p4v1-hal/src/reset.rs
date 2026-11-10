//! Why the chip last reset.

use crate::rd;

/// LP_CLKRST_RESET_CAUSE_REG (lp_clkrst_reg.h); HP core 0's cause is bits
/// [12:7] (verified on the Tab5 by tab5-os `platform/esp32p4v1.rs`).
const LP_CLKRST_RESET_CAUSE: usize = 0x5011_1010;

/// HP core 0's reset reason: the ROM's `RESET_REASON` code
/// (esp_rom/esp32p4/include/esp32p4/rom/rtc.h: 1 power-on, 3 software
/// system reset, 5 deep-sleep wake (PMU power-down), 7 / 9 / 11 / 13 / 16 /
/// 18 watchdogs, 12 software CPU reset, 15 brown-out, 22–24 USB / JTAG).
pub fn reason_code() -> u8 {
    // SAFETY: a read-only status register.
    ((unsafe { rd(LP_CLKRST_RESET_CAUSE) } >> 7) & 0x3F) as u8
}
