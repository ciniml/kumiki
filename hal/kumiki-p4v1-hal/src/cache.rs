//! Cache maintenance through the ROM's cache functions
//! (`esp_rom/esp32p4/ld/esp32p4.rom.ld`, `rom/cache.h`), the L2 line size
//! set at boot, and the cache autoload on the PSRAM. Ported from tab5-os
//! (`platform/esp32p4v1.rs`, `esp32p4v1/psram.rs`).
//!
//! Memory map (rev v1.x): L2MEM is 0x4FF0_0000..0x4FFC_0000; its top
//! 128 KiB, 0x4FFA_0000..0x4FFC_0000, is the L2 cache (both as the
//! bootloader leaves it and after [`set_l2_line`]). The linker script keeps
//! everything below 0x4FFA_0000. L2MEM sits behind the L1 caches only;
//! flash and PSRAM sit behind L1 and L2.

use crate::wr;
use crate::psram::{ALIAS, WINDOW};

// esp32p4.rom.ld:180-198; rom/cache.h:97-110.
const ROM_CACHE_GET_L2_LINE_SIZE: usize = 0x4FC0_03CC;
const ROM_CACHE_SET_L2_MODE: usize = 0x4FC0_03D4;
const ROM_CACHE_INVALIDATE_ADDR: usize = 0x4FC0_03E4;
const ROM_CACHE_WRITEBACK_ADDR: usize = 0x4FC0_03F4;
const ROM_CACHE_INVALIDATE_ALL: usize = 0x4FC0_0404;
const ROM_CACHE_WRITEBACK_ALL: usize = 0x4FC0_0414;
const CACHE_MAP_L1_ICACHE_MASK: u32 = (1 << 0) | (1 << 1);
const CACHE_MAP_L1_DCACHE: u32 = 1 << 4;
const CACHE_MAP_L2_CACHE: u32 = 1 << 5;
const CACHE_SIZE_128K: u32 = 9;
const CACHE_LINE_SIZE_128B: u32 = 4;
/// MHCR (0x7C1): return stack, predictive jumps, branch target prediction
/// (riscv/include/riscv/rv_utils.h:48-52). Off while the cache changes.
const MHCR_BRANCH_PREDICTION: usize = (1 << 4) | (1 << 5) | (1 << 12);

/// The L2 line size [`set_l2_line`] sets (the bootloader leaves 64 bytes).
pub const L2_LINE: u32 = 128;

type RomCacheFn = unsafe extern "C" fn(map: u32, addr: u32, size: u32) -> i32;

/// Writes `[addr, addr + len)` back from L1 D, then from L2 if `l2`
/// (external memory: flash and PSRAM sit behind L2).
pub fn writeback(addr: usize, len: usize, l2: bool) {
    unsafe {
        let writeback: RomCacheFn = core::mem::transmute(ROM_CACHE_WRITEBACK_ADDR);
        writeback(CACHE_MAP_L1_DCACHE, addr as u32, len as u32);
        if l2 {
            writeback(CACHE_MAP_L2_CACHE, addr as u32, len as u32);
        }
    }
}

/// Invalidates `[addr, addr + len)` in L1 I and D, and in L2 if `l2`.
/// Dirty lines in the range are dropped: write them back first if needed.
pub fn invalidate(addr: usize, len: usize, l2: bool) {
    unsafe {
        let invalidate: RomCacheFn = core::mem::transmute(ROM_CACHE_INVALIDATE_ADDR);
        invalidate(CACHE_MAP_L1_ICACHE_MASK, addr as u32, len as u32);
        invalidate(CACHE_MAP_L1_DCACHE, addr as u32, len as u32);
        if l2 {
            invalidate(CACHE_MAP_L2_CACHE, addr as u32, len as u32);
        }
    }
}

/// Invalidates `[addr, addr + len)` in L1 D only.
pub fn dcache_invalidate(addr: usize, len: usize) {
    unsafe {
        let invalidate: RomCacheFn = core::mem::transmute(ROM_CACHE_INVALIDATE_ADDR);
        invalidate(CACHE_MAP_L1_DCACHE, addr as u32, len as u32);
    }
}

/// Sets the L2 cache to 128 KiB, 8 ways, [`L2_LINE`]-byte lines and empties
/// it, as ESP-IDF's app startup does (cpu_start.c:467-487, cache_hal.c:58-
/// 77). Call once at boot, before the PSRAM is up: L2 then holds clean flash
/// lines only.
pub fn set_l2_line() {
    unsafe { set_l2_line_ram() }
}

/// Runs from RAM: the flash window goes through L2.
#[unsafe(link_section = ".ram_text")]
#[inline(never)]
unsafe fn set_l2_line_ram() {
    unsafe {
        let set_mode: unsafe extern "C" fn(u32, u32, u32) = core::mem::transmute(ROM_CACHE_SET_L2_MODE);
        let invalidate_all: unsafe extern "C" fn(u32) -> i32 = core::mem::transmute(ROM_CACHE_INVALIDATE_ALL);
        let writeback_all: unsafe extern "C" fn(u32) -> i32 = core::mem::transmute(ROM_CACHE_WRITEBACK_ALL);
        let mhcr: usize;
        core::arch::asm!("csrrc {0}, 0x7c1, {1}", out(reg) mhcr, in(reg) MHCR_BRANCH_PREDICTION);
        writeback_all(CACHE_MAP_L1_DCACHE);
        set_mode(CACHE_SIZE_128K, 8, CACHE_LINE_SIZE_128B);
        invalidate_all(CACHE_MAP_L2_CACHE);
        core::arch::asm!("csrw 0x7c1, {0}", in(reg) mhcr);
    }
}

/// The L2 line size in bytes, as the ROM reports it.
pub fn l2_line() -> u32 {
    let get: unsafe extern "C" fn() -> u32 = unsafe { core::mem::transmute(ROM_CACHE_GET_L2_LINE_SIZE) };
    unsafe { get() }
}

// Cache autoload: on a miss in a section, the cache also loads the next
// lines (cache_reg.h: L1 D 0x134-0x144, L2 0x2B4-0x2C4; control, then the
// address and size of sections 0 and 1). Off after reset and in ESP-IDF
// (cache_hal.c:49-54 keeps the ROM's setting). tab5-os doc/43 §4.
const CACHE: usize = 0x3FF1_0000;
const L1D_AUTOLOAD: usize = CACHE + 0x134;
const L2_AUTOLOAD: usize = CACHE + 0x2B4;
/// ENA, ascending, sections 0 and 1; trigger (bits 3-4) 0: on a miss.
const AUTOLOAD_WINDOWS: u32 = 1 | (1 << 8) | (1 << 9);

/// Autoload in L1 D and L2 on the first `size` bytes of both linear maps of
/// the PSRAM ([`WINDOW`] and [`ALIAS`]). Strided access (e.g. drawing
/// rotated into a framebuffer) gets slower with it, so a framebuffer is
/// best kept above `size`.
pub fn autoload(size: usize) {
    for ctrl in [L1D_AUTOLOAD, L2_AUTOLOAD] {
        unsafe {
            wr(ctrl, 0);
            wr(ctrl + 4, WINDOW as u32);
            wr(ctrl + 8, size as u32);
            wr(ctrl + 12, ALIAS as u32);
            wr(ctrl + 16, size as u32);
            wr(ctrl, AUTOLOAD_WINDOWS);
        }
    }
}
