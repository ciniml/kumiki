//! Helpers called from the generated bring-up code (binding templates in
//! `boards/bindings/*.yaml`, `p4v1` backend).

use kumiki_p4v1_hal::{cache, dsi};

/// Rotation from an optional DT `rotation` (clockwise degrees).
pub fn rotation(deg: Option<u32>) -> kumiki_gfx::Rotation {
    kumiki_gfx::Rotation::from_quarter_turns((deg.unwrap_or(0) / 90) as u8)
}

/// Frequency in Hz from an optional DT `clock-frequency`.
pub fn hz(v: Option<u32>, default: u32) -> u32 {
    v.unwrap_or(default)
}

/// Two DSI frame buffers ([`dsi::FB_BYTES`] each) at the top of PSRAM
/// (`psram` = base, size), cleared and written back. The cache autoload
/// (prefetch) is limited to the memory below them, which keeps the scan-out
/// from evicting the application's data (tab5-os doc/43 §4).
pub fn dsi_frame_buffers(psram: (usize, usize)) -> Option<[usize; 2]> {
    let (base, size) = psram;
    let need = 2 * dsi::FB_BYTES;
    // On a 1 MiB boundary, so autoload's limit is exact.
    let first = (base + size).checked_sub(need)? & !0xF_FFFF;
    if first < base {
        return None;
    }
    let fbs = [first, first + dsi::FB_BYTES];
    cache::autoload(first - base);
    for fb in fbs {
        // SAFETY: inside the mapped PSRAM, handed to the display only.
        unsafe { core::ptr::write_bytes(fb as *mut u8, 0, dsi::FB_BYTES) };
        dsi::flush(fb, dsi::FB_BYTES);
    }
    Some(fbs)
}
