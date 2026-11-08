//! Panel traits.
//!
//! A *panel* knows its command protocol (window setup, rotation, init) and
//! delegates byte transport to its bus. Surfaces such as [`crate::Display`]
//! only need [`blocking::WindowPanel`] / [`asynch::WindowPanel`].

use embedded_graphics_core::{
    geometry::{Point, Size},
    primitives::Rectangle,
};

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

    /// Whether width and height swap.
    pub const fn swaps_axes(self) -> bool {
        matches!(self, Self::Deg90 | Self::Deg270)
    }

    /// Logical size for a panel whose native size is `native`.
    pub const fn logical_size(self, native: Size) -> Size {
        if self.swaps_axes() { Size::new(native.height, native.width) } else { native }
    }

    /// Logical point → native (panel memory) point, for a panel of native
    /// size `native`. The logical image is the native one turned clockwise
    /// by `self`.
    pub const fn to_native(self, p: Point, native: Size) -> Point {
        let (w, h) = (native.width as i32, native.height as i32);
        match self {
            Self::Deg0 => p,
            Self::Deg90 => Point::new(w - 1 - p.y, p.x),
            Self::Deg180 => Point::new(w - 1 - p.x, h - 1 - p.y),
            Self::Deg270 => Point::new(p.y, h - 1 - p.x),
        }
    }

    /// Native point → logical point (inverse of [`to_native`](Self::to_native)),
    /// e.g. for touch coordinates reported in panel orientation.
    pub const fn to_logical(self, p: Point, native: Size) -> Point {
        let (w, h) = (native.width as i32, native.height as i32);
        match self {
            Self::Deg0 => p,
            Self::Deg90 => Point::new(p.y, w - 1 - p.x),
            Self::Deg180 => Point::new(w - 1 - p.x, h - 1 - p.y),
            Self::Deg270 => Point::new(h - 1 - p.y, p.x),
        }
    }

    /// Logical rectangle → native rectangle.
    pub fn rect_to_native(self, r: &Rectangle, native: Size) -> Rectangle {
        if r.size.width == 0 || r.size.height == 0 {
            return Rectangle::new(self.to_native(r.top_left, native), Size::zero());
        }
        let a = self.to_native(r.top_left, native);
        let b = self.to_native(r.top_left + Point::new(r.size.width as i32 - 1, r.size.height as i32 - 1), native);
        Rectangle::with_corners(a, b)
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
