//! SYSTIMER unit 0 (systimer_reg.h, systimer_ll.h). It counts at 16 MHz
//! from reset; its clocks are on at reset.

use crate::{rd, set_bits, wr};

const SYSTIMER_BASE: usize = 0x500E_2000;
const SYSTIMER_CONF: usize = SYSTIMER_BASE;
const SYSTIMER_CLK_EN: u32 = 1 << 31;
const SYSTIMER_UNIT0_OP: usize = SYSTIMER_BASE + 0x04;
const SYSTIMER_UNIT0_UPDATE: u32 = 1 << 30;
const SYSTIMER_UNIT0_VALUE_VALID: u32 = 1 << 29;
const SYSTIMER_UNIT0_VALUE_HI: usize = SYSTIMER_BASE + 0x40;
const SYSTIMER_UNIT0_VALUE_LO: usize = SYSTIMER_BASE + 0x44;

/// SYSTIMER ticks per microsecond.
pub const TICKS_PER_US: u64 = 16;

/// Makes sure the SYSTIMER counts (its clock is on from reset; `rt` calls
/// this before `main`). Call it once when not using `rt`.
pub fn init() {
    set_bits(SYSTIMER_CONF, SYSTIMER_CLK_EN);
}

/// Ticks (1/16 µs) since reset.
pub fn now() -> u64 {
    // systimer_hal_get_counter_value(): snapshot, then read lo/hi/lo until lo is stable.
    unsafe {
        wr(SYSTIMER_UNIT0_OP, SYSTIMER_UNIT0_UPDATE);
        while rd(SYSTIMER_UNIT0_OP) & SYSTIMER_UNIT0_VALUE_VALID == 0 {}
        let mut lo = rd(SYSTIMER_UNIT0_VALUE_LO);
        loop {
            let hi = rd(SYSTIMER_UNIT0_VALUE_HI);
            let lo2 = rd(SYSTIMER_UNIT0_VALUE_LO);
            if lo2 == lo {
                return ((hi as u64) << 32) | lo as u64;
            }
            lo = lo2;
        }
    }
}

/// Microseconds since reset.
pub fn now_us() -> u64 {
    now() / TICKS_PER_US
}

/// Busy-waits `us` microseconds.
pub fn delay_us(us: u64) {
    let end = now() + us * TICKS_PER_US;
    while now() < end {}
}

/// The CPU clock, measured: `mcycle` against SYSTIMER over 1 ms.
pub(crate) fn measure_cpu_hz() -> u32 {
    let mcycle = || {
        let c: u32;
        unsafe { core::arch::asm!("csrr {0}, mcycle", out(reg) c) };
        c
    };
    let t0 = now();
    let c0 = mcycle();
    while now() - t0 < 1000 * TICKS_PER_US {}
    let c1 = mcycle();
    let t1 = now();
    (c1.wrapping_sub(c0) as u64 * 1_000_000 * TICKS_PER_US / (t1 - t0)) as u32
}

/// Busy-wait delay (embedded-hal `DelayNs`).
#[derive(Clone, Copy, Default)]
pub struct Delay;

impl embedded_hal::delay::DelayNs for Delay {
    fn delay_ns(&mut self, ns: u32) {
        let end = now() + (ns as u64 * TICKS_PER_US).div_ceil(1000);
        while now() < end {}
    }
}
