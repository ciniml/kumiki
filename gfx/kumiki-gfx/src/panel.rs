//! Panel traits.
//!
//! A *panel* knows its command protocol (window setup, rotation, init) and
//! delegates byte transport to its bus. Surfaces such as [`crate::Display`]
//! only need [`blocking::WindowPanel`] / [`asynch::WindowPanel`].

use embedded_graphics_core::{geometry::Size, primitives::Rectangle};

use crate::pixel::PixelFormat;

/// Logical display rotation (clockwise), relative to the board's natural orientation.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Rotation {
    /// 0°
    #[default]
    Deg0 = 0,
    /// 90°
    Deg90 = 1,
    /// 180°
    Deg180 = 2,
    /// 270°
    Deg270 = 3,
}

impl Rotation {
    /// From a quarter-turn count (taken modulo 4).
    pub const fn from_quarter_turns(n: u8) -> Self {
        match n & 3 {
            0 => Self::Deg0,
            1 => Self::Deg90,
            2 => Self::Deg180,
            _ => Self::Deg270,
        }
    }
}

/// Basic panel information.
pub trait Panel {
    /// Wire pixel format the panel is currently configured for.
    type Format: PixelFormat;
    /// Visible size in the current rotation.
    fn size(&self) -> Size;
}

/// Blocking window panels.
pub mod blocking {
    use super::*;
    use crate::bus::{blocking::PanelBus, ErrorType};

    /// A panel that accepts pixels streamed into a rectangular window (MIPI-DCS style).
    pub trait WindowPanel: Panel {
        /// The bus the panel talks through.
        type Bus: PanelBus;

        /// Set the write window to `area` (already clipped to [`Panel::size`],
        /// non-empty) and enter memory-write mode. Afterwards exactly
        /// `area.size.width * area.size.height` pixels must be streamed through
        /// [`WindowPanel::bus`].
        fn begin_write(&mut self, area: &Rectangle) -> Result<(), <Self::Bus as ErrorType>::Error>;

        /// Access the bus for streaming pixel data.
        fn bus(&mut self) -> &mut Self::Bus;
    }
}

/// `async` window panels.
pub mod asynch {
    use super::*;
    use crate::bus::{asynch::PanelBus, ErrorType};

    /// `async` flavour of [`super::blocking::WindowPanel`].
    #[allow(async_fn_in_trait)]
    pub trait WindowPanel: Panel {
        /// The bus the panel talks through.
        type Bus: PanelBus;

        /// See [`super::blocking::WindowPanel::begin_write`].
        async fn begin_write(&mut self, area: &Rectangle) -> Result<(), <Self::Bus as ErrorType>::Error>;

        /// Access the bus for streaming pixel data.
        fn bus(&mut self) -> &mut Self::Bus;
    }
}
