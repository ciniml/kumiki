//! Items shared by the Espressif families.

use kumiki_class::ResetReason;

/// [`ResetReason`] from the ROM's reset reason code (`RESET_REASON` in
/// `esp_rom/<chip>/include/<chip>/rom/rtc.h`; the ESP32-S3 and ESP32-P4
/// share the numbering for the codes below).
pub fn reset_reason(code: u8) -> ResetReason {
    match code {
        1 => ResetReason::PowerOn,
        // Software reset of the digital core / the CPU.
        3 | 12 => ResetReason::Software,
        5 => ResetReason::DeepSleep,
        // MWDT0 / MWDT1 / RWDT on the core or CPU, RWDT system, super WDT.
        7 | 8 | 9 | 11 | 13 | 16 | 17 | 18 => ResetReason::Watchdog,
        15 => ResetReason::BrownOut,
        // USB-UART / USB-JTAG chip reset, JTAG CPU reset (21: S3 USB-UART).
        21..=24 => ResetReason::Software,
        c => ResetReason::Other(c),
    }
}
