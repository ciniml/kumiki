//! # kumiki-boards
//!
//! Board definitions for kumiki. Select the chip with a feature
//! (`chip-esp32s3` or `chip-p4v1`); every board of that chip is then a
//! runtime detection candidate, unless `board-*` features pin the set.
//!
//! Each family module holds the code `kumiki-dtgen` generates from the device
//! tree (`generated.rs`: detection, `init()` in dependency order) and the
//! hand-written runtime types it fills in: a `Devices` struct whose fields
//! implement the `kumiki-class` traits and can be owned separately.
#![no_std]
#![warn(missing_docs)]

#[cfg(feature = "chip-esp32s3")]
pub mod esp32s3;
#[cfg(feature = "chip-esp32s3")]
pub use esp32s3::*;

#[cfg(feature = "chip-p4v1")]
pub mod p4v1;
#[cfg(feature = "chip-p4v1")]
pub use p4v1::*;

#[cfg(not(any(feature = "chip-esp32s3", feature = "chip-p4v1")))]
compile_error!("select a chip feature: `chip-esp32s3` or `chip-p4v1`");
#[cfg(all(feature = "chip-esp32s3", feature = "chip-p4v1"))]
compile_error!("select only one chip feature");
