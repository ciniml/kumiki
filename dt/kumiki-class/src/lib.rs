//! # kumiki-class
//!
//! Board-independent *device class* traits. Board definitions (`kumiki-boards`)
//! return devices that implement these, so applications can be written once and
//! run on every board that has the capability.
//!
//! The crate has no dependencies, uses no executor and no time driver: anything
//! time-based (button debouncing) takes the current time as an argument.
#![cfg_attr(not(test), no_std)]
#![warn(missing_docs)]

pub mod buttons;

pub use buttons::{ButtonEvent, ButtonId, ButtonKind, Buttons};

/// One touch point in display coordinates (already rotated like the display).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TouchPoint {
    /// X in pixels.
    pub x: i16,
    /// Y in pixels.
    pub y: i16,
    /// Controller-assigned track id.
    pub id: u8,
}

/// Touch panel.
pub trait Touch {
    /// Error type.
    type Error: core::fmt::Debug;
    /// Read current touches into `out`; returns how many were written.
    fn read(&mut self, out: &mut [TouchPoint]) -> Result<usize, Self::Error>;
}

/// A 3-axis vector.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    /// X
    pub x: f32,
    /// Y
    pub y: f32,
    /// Z
    pub z: f32,
}

/// Accelerometer + gyroscope.
pub trait Imu {
    /// Error type.
    type Error: core::fmt::Debug;
    /// Acceleration in g.
    fn accel(&mut self) -> Result<Vec3, Self::Error>;
    /// Angular rate in degrees per second.
    fn gyro(&mut self) -> Result<Vec3, Self::Error>;
}

/// Display backlight.
pub trait Backlight {
    /// Error type.
    type Error: core::fmt::Debug;
    /// Set brightness, 0 = off, 255 = maximum.
    fn set_brightness(&mut self, level: u8) -> Result<(), Self::Error>;
}

/// Battery / power information.
pub trait Power {
    /// Error type.
    type Error: core::fmt::Debug;
    /// Battery voltage in millivolts, if a battery gauge exists.
    fn battery_mv(&mut self) -> Result<Option<u16>, Self::Error>;
    /// Whether external (USB) power is present, if detectable.
    fn vbus_present(&mut self) -> Result<Option<bool>, Self::Error>;
}
