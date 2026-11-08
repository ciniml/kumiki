/* ESP32-P4 rev v1.x, booted by the ESP-IDF 2nd stage bootloader (app image
 * at 0x10000). Layout ported from tab5-os kernel/link-esp32p4v1.ld and
 * kernel-common.ld (same author, MIT OR Apache-2.0).
 *
 * - FLASH: the cache-mapped flash window (XIP); +0x20 skips the image header.
 *   The P4 bootloader maps exactly two flash segments
 *   (bootloader_utility.c unpack_load_app: assert rom_index == 2), so
 *   "appdesc + rodata" and "text" are kept apart by a NOLOAD gap.
 * - RAM: lower L2MEM, below where the bootloader itself runs
 *   (~0x4FF2_9ED0 on this ROM / bootloader). Loaded data and the stack.
 * - RAM_HI: part of upper L2MEM, for .bss (not loaded, so it may overlap
 *   nothing the bootloader needs once the app runs). Below the L2 cache and
 *   the ROM's reserved area at the top of L2MEM. */
ENTRY(_start)

MEMORY
{
    FLASH  : ORIGIN = 0x40000020, LENGTH = 0x200000 - 0x20
    RAM    : ORIGIN = 0x4FF00000, LENGTH = 0x29000
    RAM_HI : ORIGIN = 0x4FF40000, LENGTH = 0x60000
}

STACK_SIZE = 64K;

SECTIONS
{
    /* esp_app_desc_t; must start the first flash segment. */
    .flash.appdesc : ALIGN(4) {
        KEEP(*(.flash.appdesc))
    } > FLASH

    .rodata : ALIGN(4) {
        *(.rodata .rodata.* .srodata .srodata.*)
    } > FLASH

    .text_gap (NOLOAD) : {
        . = . + 8;
        . = ALIGN(4) + 0x20;
    } > FLASH

    .text : ALIGN(4) {
        KEEP(*(.text._start))
        KEEP(*(.text.kumiki_trap))
        *(.text .text.*)
    } > FLASH

    /* Code that must not run from flash (while the cache is reconfigured). */
    .ram_text : ALIGN(64) {
        *(.ram_text .ram_text.*)
        . = ALIGN(64);
    } > RAM

    .data : ALIGN(4) {
        *(.data .data.* .sdata .sdata.*)
    } > RAM

    .stack (NOLOAD) : ALIGN(16) {
        __stack_bottom = .;
        . += STACK_SIZE;
        __stack_top = .;
    } > RAM

    .bss (NOLOAD) : ALIGN(4) {
        __bss_start = .;
        *(.sbss .sbss.* .bss .bss.* COMMON)
        . = ALIGN(4);
        __bss_end = .;
    } > RAM_HI

    /DISCARD/ : { *(.eh_frame .eh_frame_hdr) }
}
