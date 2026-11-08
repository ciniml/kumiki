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

/// Calendar date and time (no time zone; whatever the application sets).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DateTime {
    /// Full year (2000–2099 on most RTCs).
    pub year: u16,
    /// 1–12.
    pub month: u8,
    /// 1–31.
    pub day: u8,
    /// 0–6, 0 = Sunday.
    pub weekday: u8,
    /// 0–23.
    pub hour: u8,
    /// 0–59.
    pub minute: u8,
    /// 0–59.
    pub second: u8,
}

/// Real-time clock.
pub trait Rtc {
    /// Error type.
    type Error: core::fmt::Debug;
    /// Current date and time.
    fn datetime(&mut self) -> Result<DateTime, Self::Error>;
    /// Set the date and time.
    fn set_datetime(&mut self, t: &DateTime) -> Result<(), Self::Error>;
    /// Whether the clock lost power (backup voltage low) since it was last
    /// set, i.e. the time is not trustworthy.
    fn power_lost(&mut self) -> Result<bool, Self::Error>;
}

/// A backlight that is only switched on and off by a GPIO.
pub struct GpioBacklight<P>(pub P);

impl<P: embedded_hal::digital::OutputPin> Backlight for GpioBacklight<P> {
    type Error = P::Error;
    fn set_brightness(&mut self, level: u8) -> Result<(), P::Error> {
        if level > 0 { self.0.set_high() } else { self.0.set_low() }
    }
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
