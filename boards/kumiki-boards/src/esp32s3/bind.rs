//! Helpers called from the generated bring-up code (binding templates in
//! `boards/bindings/*.yaml`). Keep them small and data-driven: anything
//! board-specific belongs in the device tree.

use embedded_hal::{delay::DelayNs, digital::OutputPin};
use esp_hal::time::Rate;
use kumiki_gfx_panels::DcsConfig;

/// Pulse an active-low reset line: low 10 ms, high, wait 120 ms.
pub fn reset_pulse(pin: &mut impl OutputPin, delay: &mut impl DelayNs) {
    let _ = pin.set_low();
    delay.delay_ms(10);
    let _ = pin.set_high();
    delay.delay_ms(120);
}

/// DcsConfig from generated panel constants.
pub fn dcs_config(width: u32, height: u32, offset: &[u32], invert: bool, rgb_order: Option<&str>) -> DcsConfig {
    DcsConfig {
        panel_size: (width as u16, height as u16),
        offset: (offset.first().copied().unwrap_or(0) as u16, offset.get(1).copied().unwrap_or(0) as u16),
        offset_rotation: 0,
        invert,
        rgb_order: rgb_order == Some("rgb"),
    }
}

/// Whether the first entry of a generated `*_GPIOS` constant is active-low.
pub fn active_low(g: &[super::generated::Gpio]) -> bool {
    use super::generated::Gpio;
    matches!(g.first(), Some(Gpio::Chip { active_low: true, .. } | Gpio::Expander { active_low: true, .. }))
}

/// Frequency from an optional DT `clock-frequency`.
pub fn hz(v: Option<u32>, default: u32) -> Rate {
    Rate::from_hz(v.unwrap_or(default))
}

/// Bring PSRAM up in the board's DT mode (`kumiki,mode`: `quad` / `octal`)
/// and map all of it for the program's lifetime. `None`: it did not answer.
pub fn map_psram(psram: esp_hal::peripherals::PSRAM<'static>, mode: &str) -> Option<&'static mut [u8]> {
    use esp_hal::psram::{Psram, PsramConfig, PsramMode};
    let mode = match mode {
        "quad" => PsramMode::QuadSpi,
        "octal" => PsramMode::OctalSpi,
        _ => PsramMode::Auto, // not reached: dtgen checks the mode
    };
    let psram = Psram::new(psram, PsramConfig { mode, ..PsramConfig::default() });
    let (ptr, size) = psram.raw_parts();
    core::mem::forget(psram);
    // SAFETY: mapped for the program's lifetime; handed out once, here.
    (size > 0).then(|| unsafe { core::slice::from_raw_parts_mut(ptr, size) })
}

/// Take `len` bytes from the start of `region`, leaving the rest in it
/// (`None` once nothing is left). `None` if it is missing or too small.
pub fn carve(region: &mut Option<&'static mut [u8]>, len: usize) -> Option<&'static mut [u8]> {
    let r = region.take()?;
    if r.len() < len {
        *region = Some(r);
        return None;
    }
    let (head, rest) = r.split_at_mut(len);
    *region = (!rest.is_empty()).then_some(rest);
    Some(head)
}
