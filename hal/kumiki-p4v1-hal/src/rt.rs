//! Entry point and runtime.
//!
//! The ESP-IDF 2nd stage bootloader loads the RAM segments, maps the two
//! flash segments and jumps to `_start` (machine mode, interrupts off).
//! `_start` enables the FPU, sets the stack, zeroes `.bss`, installs the
//! trap vector and calls the application's entry, declared with
//! [`entry!`](crate::entry).
//!
//! Before the application runs, the watchdogs the bootloader left armed
//! (RWDT, the timer-group MWDTs) are turned off.

core::arch::global_asm!(
    r#"
    .section .text._start, "ax"
    .global _start
_start:
    csrci mstatus, 8            # MIE = 0
    li t0, 0x2000               # mstatus.FS = Initial: enable the FPU
    csrs mstatus, t0
    csrw fcsr, zero
    la sp, __stack_top
    la t0, __bss_start
    la t1, __bss_end
1:  bgeu t0, t1, 2f
    sw zero, 0(t0)
    addi t0, t0, 4
    j 1b
2:  la t0, kumiki_trap
    ori t0, t0, 3               # CLIC mode, as the bootloader left it
    csrw mtvec, t0
    call {start}
3:  wfi
    j 3b

    .section .text.kumiki_trap, "ax"
    .balign 256
    .global kumiki_trap
kumiki_trap:
    # Every trap is fatal here (no interrupts are enabled): report it on a
    # fresh stack.
    la sp, __stack_top
    csrr a0, mcause
    csrr a1, mepc
    csrr a2, mtval
    call {fatal}
4:  j 4b
    "#,
    start = sym start_rust,
    fatal = sym fatal_trap,
);

unsafe extern "Rust" {
    fn __kumiki_main() -> !;
}

unsafe extern "C" fn start_rust() -> ! {
    // Keep the app descriptor even when nothing else references it.
    unsafe { core::ptr::read_volatile(&APP_DESC.magic_word) };
    crate::wdt::disable_all();
    crate::time::init();
    unsafe { __kumiki_main() }
}

unsafe extern "C" fn fatal_trap(mcause: u32, mepc: u32, mtval: u32) -> ! {
    crate::println!("\n*** trap: mcause={mcause:#010x} mepc={mepc:#010x} mtval={mtval:#010x}");
    loop {
        core::hint::spin_loop();
    }
}

/// Declares the application's entry point (`fn() -> !`).
///
/// ```ignore
/// kumiki_p4v1_hal::entry!(main);
/// fn main() -> ! { loop {} }
/// ```
#[macro_export]
macro_rules! entry {
    ($path:path) => {
        #[unsafe(export_name = "__kumiki_main")]
        pub fn __kumiki_entry() -> ! {
            let f: fn() -> ! = $path;
            f()
        }
    };
}

/// ESP-IDF `esp_app_desc_t` (esp_app_format/include/esp_app_desc.h). The
/// bootloader checks the eFuse block revision range; espflash reads the MMU
/// page size.
#[repr(C)]
pub struct AppDesc {
    magic_word: u32,
    secure_version: u32,
    reserv1: [u32; 2],
    version: [u8; 32],
    project_name: [u8; 32],
    time: [u8; 16],
    date: [u8; 16],
    idf_ver: [u8; 32],
    app_elf_sha256: [u8; 32],
    min_efuse_blk_rev_full: u16,
    max_efuse_blk_rev_full: u16,
    mmu_page_size: u8,
    reserv3: [u8; 3],
    reserv2: [u32; 18],
}

const _: () = assert!(core::mem::size_of::<AppDesc>() == 256);

const fn cstr<const N: usize>(s: &str) -> [u8; N] {
    let b = s.as_bytes();
    let mut out = [0u8; N];
    let mut i = 0;
    while i < b.len() && i < N - 1 {
        out[i] = b[i];
        i += 1;
    }
    out
}

#[used]
#[unsafe(link_section = ".flash.appdesc")]
static APP_DESC: AppDesc = AppDesc {
    magic_word: 0xABCD_5432,
    secure_version: 0,
    reserv1: [0; 2],
    version: cstr(env!("CARGO_PKG_VERSION")),
    project_name: cstr("kumiki"),
    time: [0; 16],
    date: [0; 16],
    idf_ver: cstr("kumiki-p4v1-hal"),
    app_elf_sha256: [0; 32],
    // Same range as an ESP-IDF build for rev v1.x (the Tab5 reports eFuse
    // block revision v0.3).
    min_efuse_blk_rev_full: 0,
    max_efuse_blk_rev_full: 199,
    mmu_page_size: 16, // 64 KiB
    reserv3: [0; 3],
    reserv2: [0; 18],
};
