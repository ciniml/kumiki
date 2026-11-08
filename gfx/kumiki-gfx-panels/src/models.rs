//! Panel models: geometry and init tables.
//!
//! Init tables use the LovyanGFX/M5GFX encoding:
//! `cmd, n_args | DELAY_FLAG, args..., [delay_ms if DELAY_FLAG]`, terminated by `0xFF, 0xFF`.
//!
//! The tables below are ported from M5GFX (MIT, Copyright (c) 2021 M5Stack) /
//! LovyanGFX (BSD-2-Clause, Copyright (c) 2020 lovyan03):
//! `src/lgfx/v1/panel/Panel_ILI9342.hpp` and `Panel_GC9A01.hpp` (`Panel_GC9107`).
//! See THIRD_PARTY_NOTICES.md.

/// Flag OR-ed into the argument count: a delay byte (ms) follows the arguments.
pub const DELAY: u8 = 0x80;

/// A MIPI-DCS panel controller model.
pub trait Model {
    /// Human-readable name.
    const NAME: &'static str;
    /// Controller memory size (width, height) in the native orientation.
    const MEMORY_SIZE: (u16, u16);
    /// Default visible panel size (width, height); boards may override.
    const PANEL_SIZE: (u16, u16) = Self::MEMORY_SIZE;
    /// Init table (see module docs).
    const INIT: &'static [u8];
    /// Default colour inversion.
    const INVERT: bool = false;
    /// GC9xxx quirk: when the axes are swapped, RASET must be reset to the full
    /// range before CASET (see M5GFX `Panel_GC9xxx::setWindow`).
    const RESET_RASET_WHEN_SWAPPED: bool = false;
    /// Expected RDDID (0x04) response: `(mask, value)`.
    const ID: Option<(u32, u32)> = None;
}

/// Ilitek ILI9342C (320×240). Used on M5Stack CoreS3 / Core2.
pub struct Ili9342c;

impl Model for Ili9342c {
    const NAME: &'static str = "ILI9342C";
    const MEMORY_SIZE: (u16, u16) = (320, 240);
    #[rustfmt::skip]
    const INIT: &'static [u8] = &[
        0xC8, 3, 0xFF, 0x93, 0x42,          // SETEXTC
        0xC0, 2, 0x12, 0x12,                // PWCTR1
        0xC1, 1, 0x03,                      // PWCTR2
        0xC5, 1, 0xF2,                      // VMCTR1
        0xB0, 1, 0xE0,
        0xF6, 3, 0x01, 0x00, 0x00,
        0xE0, 15, 0x00, 0x0C, 0x11, 0x04, 0x11, 0x08, 0x37, 0x89, 0x4C, 0x06, 0x0C, 0x0A, 0x2E, 0x34, 0x0F, // GMCTRP1
        0xE1, 15, 0x00, 0x0B, 0x11, 0x05, 0x13, 0x09, 0x33, 0x67, 0x48, 0x07, 0x0E, 0x0B, 0x2E, 0x33, 0x0F, // GMCTRN1
        0xB6, 4, 0x08, 0x82, 0x1D, 0x04,    // DFUNCTR
        0x38, 0,                            // IDMOFF
        0x29, 0,                            // DISPON
        0x11, DELAY, 120,                   // SLPOUT
        0xFF, 0xFF,
    ];
    const ID: Option<(u32, u32)> = Some((0xFF, 0xE3));
}

/// GalaxyCore GC9107 (128×160 memory). Used on M5Stack AtomS3 (128×128 visible, y offset 32).
pub struct Gc9107;

impl Model for Gc9107 {
    const NAME: &'static str = "GC9107";
    const MEMORY_SIZE: (u16, u16) = (128, 160);
    #[rustfmt::skip]
    const INIT: &'static [u8] = &[
        0xFE, DELAY, 5,
        0xEF, DELAY, 5,
        0xB0, 1, 0xC0,
        0xB2, 1, 0x2F,
        0xB3, 1, 0x03,
        0xB6, 1, 0x19,
        0xB7, 1, 0x01,
        0xAC, 1, 0xCB,
        0xAB, 1, 0x0E,
        0xB4, 1, 0x04,
        0xA8, 1, 0x19,
        0xB8, 1, 0x08,
        0xE8, 1, 0x24,
        0xE9, 1, 0x48,
        0xEA, 1, 0x22,
        0xC6, 1, 0x30,
        0xC7, 1, 0x18,
        0xF0, 14, 0x1F, 0x28, 0x04, 0x3E, 0x2A, 0x2E, 0x20, 0x00, 0x0C, 0x06, 0x00, 0x1C, 0x1F, 0x0F,
        0xF1, 14, 0x00, 0x2D, 0x2F, 0x3C, 0x6F, 0x1C, 0x0B, 0x00, 0x00, 0x00, 0x07, 0x0D, 0x11, 0x0F,
        0x11, DELAY, 120,                   // SLPOUT
        0x29, 0,                            // DISPON
        0xFF, 0xFF,
    ];
    const RESET_RASET_WHEN_SWAPPED: bool = true;
    const ID: Option<(u32, u32)> = Some((0xFF_FFFF, 0x07_9100));
}

/// Iterate `(cmd, args, delay_ms)` entries of an init table.
pub fn init_entries(table: &'static [u8]) -> impl Iterator<Item = (u8, &'static [u8], u8)> {
    let mut i = 0;
    core::iter::from_fn(move || {
        if i + 1 >= table.len() || (table[i] == 0xFF && table[i + 1] == 0xFF) {
            return None;
        }
        let cmd = table[i];
        let n = (table[i + 1] & !DELAY) as usize;
        let has_delay = table[i + 1] & DELAY != 0;
        let args = &table[i + 2..i + 2 + n];
        i += 2 + n;
        let delay = if has_delay {
            let d = table[i];
            i += 1;
            d
        } else {
            0
        };
        Some((cmd, args, delay))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_parse_to_end() {
        let v: Vec<_> = init_entries(Ili9342c::INIT).collect();
        assert_eq!(v.first().unwrap().0, 0xC8);
        assert_eq!(v.last().unwrap(), &(0x11, &[][..], 120));
        let g: Vec<_> = init_entries(Gc9107::INIT).collect();
        assert_eq!(g[0], (0xFE, &[][..], 5));
        assert_eq!(g.last().unwrap().0, 0x29);
    }
}
