//! Minimal HAL for the ESP32-P4 **chip revision v1.x** (M5Stack Tab5).
//!
//! esp-hal supports only rev v3.0+ and will not accept v1.x support
//! (esp-hal roadmap #3962), so this crate drives the registers directly,
//! after ESP-IDF's `components/soc/esp32p4/register/hw_ver1` headers. Most
//! of it is ported from tab5-os (`kernel/src/platform/esp32p4v1*`, same
//! author, MIT OR Apache-2.0), where it runs on a real Tab5.
//!
//! Scope: only what kumiki's Tab5 board definition needs. Blocking,
//! single-core, polled.
//!
//! - [`rt`]: entry point, app descriptor, linker script, fatal trap report
//! - [`usj`]: USB Serial/JTAG console ([`println!`])
//! - [`time`]: SYSTIMER clock and [`time::Delay`]
//! - [`gpio`]: pins and embedded-hal digital traits
//! - [`i2c`]: I²C master with embedded-hal's `I2c`
//! - [`clock`]: PVT and the 360 MHz CPU clock
//! - [`cache`]: L1/L2 cache maintenance (ROM functions), L2 line size,
//!   autoload
//! - [`psram`]: the 32 MiB HEX PSRAM at 200 MHz, mapped at
//!   [`psram::WINDOW`]
//! - [`dsi`]: MIPI-DSI video output (720x1280 RGB565) from a framebuffer
//! - [`ppa`]: PPA / 2D-DMA rectangle fill and copy

#![no_std]

pub mod cache;
pub mod clock;
pub mod dsi;
#[cfg(feature = "gfx")]
pub mod fb;
pub mod gpio;
pub mod i2c;
pub mod ppa;
pub mod psram;
pub mod rt;
pub mod time;
pub mod usj;
mod wdt;

/// Logs a line from inside the HAL (`[module] ...`), with the `log`
/// feature.
#[cfg(feature = "log")]
macro_rules! log {
    ($($arg:tt)*) => {{
        $crate::print!("[{}] ", module_path!().trim_start_matches("kumiki_p4v1_hal::"));
        $crate::println!($($arg)*);
    }};
}
#[cfg(not(feature = "log"))]
macro_rules! log {
    ($($arg:tt)*) => {{
        if false {
            let _ = format_args!($($arg)*);
        }
    }};
}
pub(crate) use log;

pub use embedded_hal;

/// Volatile 32-bit register read.
///
/// # Safety
/// `addr` must be a valid, readable MMIO register.
#[inline(always)]
pub(crate) unsafe fn rd(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(addr as *const u32) }
}

/// Volatile 32-bit register write.
///
/// # Safety
/// `addr` must be a valid, writable MMIO register.
#[inline(always)]
pub(crate) unsafe fn wr(addr: usize, v: u32) {
    unsafe { core::ptr::write_volatile(addr as *mut u32, v) }
}

pub(crate) fn set_bits(addr: usize, bits: u32) {
    unsafe { wr(addr, rd(addr) | bits) }
}

pub(crate) fn clear_bits(addr: usize, bits: u32) {
    unsafe { wr(addr, rd(addr) & !bits) }
}

/// Sets the `width`-bit field at `shift` to `v`.
pub(crate) fn set_field(addr: usize, mask: u32, shift: u32, v: u32) {
    unsafe { wr(addr, (rd(addr) & !mask) | ((v << shift) & mask)) }
}

/// Sets the `width`-bit field at `shift` to `v` (tab5-os's `set_field`
/// argument order).
pub(crate) fn set_bits_field(addr: usize, shift: u32, width: u32, v: u32) {
    let mask = ((1u32 << width) - 1) << shift;
    unsafe { wr(addr, (rd(addr) & !mask) | ((v << shift) & mask)) }
}

/// Polls `done` for up to `us` microseconds; false on timeout.
pub(crate) fn wait_us(us: u64, mut done: impl FnMut() -> bool) -> bool {
    let end = time::now() + us * time::TICKS_PER_US;
    while !done() {
        if time::now() > end {
            return false;
        }
    }
    true
}

/// The peripherals this HAL drives, each taken once.
pub struct Peripherals {
    /// GPIO pins.
    pub pins: gpio::Pins,
    /// I²C controller 0.
    pub i2c0: i2c::I2c0,
    /// I²C controller 1.
    pub i2c1: i2c::I2c1,
}

static mut TAKEN: bool = false;

/// Takes the peripherals. Returns `None` after the first call.
pub fn take() -> Option<Peripherals> {
    // Single core, called from the main flow only.
    unsafe {
        if TAKEN {
            return None;
        }
        TAKEN = true;
    }
    Some(Peripherals { pins: gpio::Pins::new(), i2c0: i2c::I2c0(()), i2c1: i2c::I2c1(()) })
}

/// Chip information read at run time.
#[derive(Debug, Clone, Copy)]
pub struct ChipInfo {
    /// `misa`.
    pub misa: u32,
    /// The CPU clock in Hz, measured against SYSTIMER.
    pub cpu_hz: u32,
    /// The factory MAC address.
    pub mac: [u8; 6],
}

/// Reads [`ChipInfo`] (takes about 1 ms).
pub fn chip_info() -> ChipInfo {
    let misa: u32;
    unsafe { core::arch::asm!("csrr {0}, misa", out(reg) misa) };
    // EFUSE_RD_MAC_SYS_0 / _1 (efuse_reg.h): MAC[0..4] little-endian, then MAC[4..6].
    let (lo, hi) = unsafe { (rd(0x5012_D044), rd(0x5012_D048)) };
    let mac = [(hi >> 8) as u8, hi as u8, (lo >> 24) as u8, (lo >> 16) as u8, (lo >> 8) as u8, lo as u8];
    ChipInfo { misa, cpu_hz: time::measure_cpu_hz(), mac }
}
