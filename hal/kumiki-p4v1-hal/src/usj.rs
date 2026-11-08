//! USB Serial/JTAG output (usb_serial_jtag_reg.h). Works straight after
//! the bootloader without any set-up. The default [`console`](crate::console)
//! sink with feature `usj-console`.

use crate::{rd, time, wr};

const USJ_BASE: usize = 0x500D_2000;
const USJ_EP1: usize = USJ_BASE;
const USJ_EP1_CONF: usize = USJ_BASE + 0x4;
const USJ_WR_DONE: u32 = 1 << 0;
const USJ_SERIAL_IN_EP_DATA_FREE: u32 = 1 << 1;
/// How long to wait for FIFO room before deciding nobody reads the port.
/// Longer than the host's 1 ms polling interval.
const TIMEOUT_US: u64 = 50_000;

static mut STALLED: bool = false;

/// Writes `bytes`. While no host reads the port, bytes are dropped after
/// one timeout instead of slowing every write down.
pub fn write(bytes: &[u8]) {
    // Single core, no interrupts.
    let stalled = unsafe { &mut *core::ptr::addr_of_mut!(STALLED) };
    let free = || unsafe { rd(USJ_EP1_CONF) & USJ_SERIAL_IN_EP_DATA_FREE != 0 };
    for &b in bytes {
        if !free() {
            if !*stalled {
                let end = time::now() + TIMEOUT_US * time::TICKS_PER_US;
                while !free() && time::now() < end {}
            }
            if !free() {
                *stalled = true;
                continue;
            }
        }
        *stalled = false;
        unsafe { wr(USJ_EP1, b as u32) };
    }
    unsafe { wr(USJ_EP1_CONF, USJ_WR_DONE) };
}
