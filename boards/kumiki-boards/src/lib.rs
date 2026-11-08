//! # kumiki-boards
//!
//! Board definitions for kumiki. Select the chip with a feature
//! (`chip-esp32s3`); every board of that chip is then a runtime detection
//! candidate, unless `board-*` features pin the set.
//!
//! This is the hand-written form of what `kumiki-dtgen` will generate from the
//! device tree in milestone M3 (doc/07-design-hwdesc.md §6): a detection
//! function, an `init()` that brings devices up in dependency order, and a
//! `Devices` struct whose fields implement the `kumiki-class` traits and can be
//! owned separately.
#![no_std]
#![warn(missing_docs)]

#[cfg(feature = "chip-esp32s3")]
pub mod esp32s3;

#[cfg(feature = "chip-esp32s3")]
pub use esp32s3::*;

#[cfg(not(feature = "chip-esp32s3"))]
compile_error!("select a chip feature, e.g. `chip-esp32s3`");
