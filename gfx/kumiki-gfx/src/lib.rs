//! # kumiki-gfx
//!
//! HAL-independent graphics core of the kumiki project.
//!
//! * [`pixel`] — wire-order pixel formats and fused conversion.
//! * [`bus`] — panel bus traits (blocking / `async`), with a pipelined
//!   `write_with` so DMA buses can overlap conversion and transfer.
//! * [`panel`] — window-panel traits.
//! * [`Display`] — embedded-graphics [`DrawTarget`](embedded_graphics_core::draw_target::DrawTarget)
//!   on top of a panel, plus the [`FastDraw`] / [`AsyncFastDraw`] fast paths.
//! * [`Sprite`] — in-RAM surface in wire order with keyed-transparency push.
//!
//! The crate depends only on `embedded-graphics-core`, `embedded-hal` and
//! `embedded-hal-async`; it uses no executor and no allocator.
#![cfg_attr(not(test), no_std)]
#![warn(missing_docs)]

pub mod buffered;
pub mod bus;
pub mod display;
pub mod draw;
pub mod panel;
pub mod pixel;
pub mod sprite;

pub use buffered::BufferedDisplay;
pub use display::Display;
pub use draw::{AsyncFastDraw, FastDraw};
pub use panel::Rotation;
pub use pixel::{PixelFormat, Rgb565Be, Rgb565Le, Rgb888Fmt};
pub use sprite::Sprite;

#[cfg(test)]
mod tests;
