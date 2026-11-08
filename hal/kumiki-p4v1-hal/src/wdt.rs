//! Watchdogs left running by the bootloader (lp_wdt_reg.h,
//! timer_group_reg.h; tab5-os `platform/esp32p4v1.rs`).

use crate::{rd, wr};

const LP_WDT_BASE: usize = 0x5011_6000;
const LP_WDT_CONFIG0: usize = LP_WDT_BASE;
const LP_WDT_WPROTECT: usize = LP_WDT_BASE + 0x18;
const WDT_WKEY: u32 = 0x50D8_3AA1;

const TIMG_BASES: [usize; 2] = [0x500C_2000, 0x500C_3000];
const TIMG_WDTCONFIG0: usize = 0x48;
const TIMG_WDTWPROTECT: usize = 0x64;
const TIMG_WDT_EN: u32 = 1 << 31;
const TIMG_WDT_FLASHBOOT_MOD_EN: u32 = 1 << 14;
/// The MWDT takes a changed configuration only with this set (mwdt_ll.h).
const TIMG_WDT_CONF_UPDATE_EN: u32 = 1 << 22;

/// Turns off the RTC watchdog and both timer-group watchdogs. (The super
/// watchdog is fed automatically; the bootloader sets its auto-feed.)
pub fn disable_all() {
    unsafe {
        wr(LP_WDT_WPROTECT, WDT_WKEY);
        wr(LP_WDT_CONFIG0, 0);
        wr(LP_WDT_WPROTECT, 0);
        for base in TIMG_BASES {
            wr(base + TIMG_WDTWPROTECT, WDT_WKEY);
            wr(base + TIMG_WDTCONFIG0, rd(base + TIMG_WDTCONFIG0) & !(TIMG_WDT_EN | TIMG_WDT_FLASHBOOT_MOD_EN));
            wr(base + TIMG_WDTCONFIG0, rd(base + TIMG_WDTCONFIG0) | TIMG_WDT_CONF_UPDATE_EN);
            wr(base + TIMG_WDTWPROTECT, 0);
        }
    }
}
