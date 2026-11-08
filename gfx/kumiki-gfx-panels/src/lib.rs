//! # kumiki-gfx-panels
//!
//! MIPI-DCS panel drivers for [`kumiki_gfx`]: one generic [`DcsPanel`] plus
//! per-controller [`Model`]s (init tables and geometry).
#![cfg_attr(not(test), no_std)]
#![warn(missing_docs)]

pub mod dcs;
pub mod models;

pub use dcs::{DcsConfig, DcsPanel, NoPin, PanelError};
pub use models::{Co5300, Gc9107, Ili9342c, Model};

#[cfg(test)]
mod tests {
    use embedded_graphics_core::{geometry::Point, geometry::Size, primitives::Rectangle};
    use kumiki_gfx::{
        bus::{blocking::PanelBus, ErrorType},
        panel::{blocking::WindowPanel, Panel},
        Rotation,
    };

    use super::*;

    #[derive(Default)]
    struct Rec(Vec<(u8, Vec<u8>)>);
    impl ErrorType for Rec {
        type Error = core::convert::Infallible;
    }
    impl PanelBus for Rec {
        fn command(&mut self, cmd: u8, params: &[u8]) -> Result<(), Self::Error> {
            self.0.push((cmd, params.to_vec()));
            Ok(())
        }
        fn write_with<F: FnMut(&mut [u8])>(&mut self, _t: usize, _u: usize, _f: F) -> Result<(), Self::Error> {
            Ok(())
        }
        fn flush(&mut self) -> Result<(), Self::Error> {
            Ok(())
        }
    }

    struct NoDelay;
    impl embedded_hal::delay::DelayNs for NoDelay {
        fn delay_ns(&mut self, _ns: u32) {}
    }

    #[test]
    fn window_cache_skips_unchanged_axes() {
        let mut p = DcsPanel::new(Rec::default(), Ili9342c, DcsConfig::for_model::<Ili9342c>(), None::<NoPin>);
        let a = Rectangle::new(Point::new(10, 20), Size::new(5, 6));
        p.begin_write(&a).unwrap();
        p.begin_write(&a).unwrap();
        let cmds: Vec<u8> = p.bus().0.iter().map(|c| c.0).collect();
        assert_eq!(cmds, [0x2A, 0x2B, 0x2C, 0x2C]);
        assert_eq!(p.bus().0[0].1, [0, 10, 0, 14]);
        assert_eq!(p.bus().0[1].1, [0, 20, 0, 25]);
    }

    #[test]
    fn atoms3_offset_and_rotation() {
        let cfg = DcsConfig { panel_size: (128, 128), offset: (0, 32), ..DcsConfig::for_model::<Gc9107>() };
        let mut p = DcsPanel::new(Rec::default(), Gc9107, cfg, None::<NoPin>);
        assert_eq!(p.size(), Size::new(128, 128));
        p.begin_write(&Rectangle::new(Point::zero(), Size::new(128, 128))).unwrap();
        assert_eq!(p.bus().0[1].1, [0, 32, 0, 159]);
        p.bus().0.clear();
        p.set_rotation(Rotation::Deg180).unwrap();
        p.begin_write(&Rectangle::new(Point::zero(), Size::new(128, 128))).unwrap();
        // 180°: rows counted from the other end → rowstart = 160 - (128 + 32) = 0
        let raset = p.bus().0.iter().find(|c| c.0 == 0x2B).unwrap().1.clone();
        assert_eq!(raset, [0, 0, 0, 127]);
    }

    #[test]
    fn init_sends_table_then_format() {
        let mut cfg = DcsConfig::for_model::<Ili9342c>();
        cfg.invert = true;
        let mut p = DcsPanel::new(Rec::default(), Ili9342c, cfg, Some(NoPin));
        p.init(&mut NoDelay).unwrap();
        let cmds: Vec<u8> = p.bus().0.iter().map(|c| c.0).collect();
        assert_eq!(cmds[0], 0xC8);
        assert_eq!(&cmds[cmds.len() - 3..], &[0x3A, 0x21, 0x36]);
        assert_eq!(p.bus().0.last().unwrap().1, [0x08]); // BGR, rotation 0
    }
}
