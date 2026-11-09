//! Trait impls of the `Any*` enums of the ESP32-P4 rev v1 family. As in
//! the ESP32-S3 family, the enums and their dispatch macros are generated
//! into [`super::generated`]; same method names, so applications can be
//! written against either family.

use embedded_graphics_core::{
    draw_target::DrawTarget,
    geometry::{Dimensions, OriginDimensions, Point, Size},
    pixelcolor::Rgb565,
    primitives::Rectangle,
    Pixel,
};
use kumiki_class::{Backlight, DateTime, Imu, Microphone, Power, Rtc, Speaker, Touch, TouchPoint, Vec3};
use kumiki_gfx::{FastDraw, PixelFormat, Rgb565Le};
use kumiki_p4v1_hal::i2c;

use super::{AnyBacklight, AnyDisplay, AnyImu, AnyMicrophone, AnyPower, AnyRtc, AnySpeaker, AnyTouch};

/// Display error: a 2D-engine (PPA) failure while presenting.
pub type DisplayError = kumiki_p4v1_hal::fb::PpaError;

/// Error of the `Any*` devices.
#[derive(Debug)]
pub enum AnyError {
    /// I²C failure.
    I2c(i2c::Error),
    /// The device reported an unexpected state.
    Device,
}

impl From<i2c::Error> for AnyError {
    fn from(e: i2c::Error) -> Self {
        AnyError::I2c(e)
    }
}

impl From<core::convert::Infallible> for AnyError {
    fn from(e: core::convert::Infallible) -> Self {
        match e {}
    }
}

impl From<kumiki_bmi270::Error<i2c::Error>> for AnyError {
    fn from(e: kumiki_bmi270::Error<i2c::Error>) -> Self {
        match e {
            kumiki_bmi270::Error::I2c(e) => AnyError::I2c(e),
            _ => AnyError::Device,
        }
    }
}

// ---- display ------------------------------------------------------------------------

impl OriginDimensions for AnyDisplay {
    fn size(&self) -> Size {
        any_display!(self, d => d.size())
    }
}

impl DrawTarget for AnyDisplay {
    type Color = Rgb565;
    type Error = DisplayError;

    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(&mut self, pixels: I) -> Result<(), DisplayError> {
        any_display!(self, d => d.draw_iter(pixels).map_err(Into::into))
    }
    fn fill_contiguous<I: IntoIterator<Item = Rgb565>>(&mut self, area: &Rectangle, colors: I) -> Result<(), DisplayError> {
        any_display!(self, d => d.fill_contiguous(area, colors).map_err(Into::into))
    }
    fn fill_solid(&mut self, area: &Rectangle, color: Rgb565) -> Result<(), DisplayError> {
        any_display!(self, d => d.fill_solid(area, color).map_err(Into::into))
    }
    fn clear(&mut self, color: Rgb565) -> Result<(), DisplayError> {
        any_display!(self, d => d.clear(color).map_err(Into::into))
    }
}

impl FastDraw for AnyDisplay {
    type Format = Rgb565Le;
    fn fill_rect(&mut self, area: &Rectangle, color: Rgb565) -> Result<(), DisplayError> {
        any_display!(self, d => FastDraw::fill_rect(d, area, color).map_err(Into::into))
    }
    fn blit<S: PixelFormat>(&mut self, area: &Rectangle, src: &[u8], stride_px: usize) -> Result<(), DisplayError> {
        any_display!(self, d => d.blit::<S>(area, src, stride_px).map_err(Into::into))
    }
}

impl AnyDisplay {
    /// The area guaranteed to be visible (the whole screen here).
    pub fn safe_area(&self) -> Rectangle {
        self.bounding_box()
    }

    /// A touch report (panel-native coordinates) in screen coordinates,
    /// following the display's rotation (DT `rotation`).
    pub fn touch_to_screen(&self, p: &TouchPoint) -> Point {
        let n = Point::new(p.x as i32, p.y as i32);
        any_display!(self, d => d.native_to_logical(n))
    }

    /// Change the rotation (clockwise from the panel's native orientation).
    /// Redraw everything afterwards.
    pub fn set_rotation(&mut self, r: kumiki_gfx::Rotation) {
        any_display!(self, d => d.set_rotation(r))
    }

    /// Present the frame (cache write-back of the changed area, buffer swap).
    pub fn flush(&mut self) -> Result<(), DisplayError> {
        any_display!(self, d => d.flush())
    }

    /// Keep the scan-out going while not presenting for a while (call at
    /// least every ~200 ms; `flush` does it too).
    pub fn service(&mut self) {
        any_display!(self, d => d.panel_mut().service())
    }
}

// ---- other classes ---------------------------------------------------------------

impl Backlight for AnyBacklight {
    type Error = AnyError;
    fn set_brightness(&mut self, level: u8) -> Result<(), AnyError> {
        any_backlight!(self, b => b.set_brightness(level).map_err(AnyError::from), none => Ok(()))
    }
}

impl Touch for AnyTouch {
    type Error = AnyError;
    fn read(&mut self, out: &mut [TouchPoint]) -> Result<usize, AnyError> {
        any_touch!(self, t => t.read(out).map_err(AnyError::from))
    }
}

impl Imu for AnyImu {
    type Error = AnyError;
    fn accel(&mut self) -> Result<Vec3, AnyError> {
        any_imu!(self, m => m.accel().map_err(AnyError::from))
    }
    fn gyro(&mut self) -> Result<Vec3, AnyError> {
        any_imu!(self, m => m.gyro().map_err(AnyError::from))
    }
}

impl Rtc for AnyRtc {
    type Error = AnyError;
    fn datetime(&mut self) -> Result<DateTime, AnyError> {
        any_rtc!(self, r => r.datetime().map_err(AnyError::from))
    }
    fn set_datetime(&mut self, t: &DateTime) -> Result<(), AnyError> {
        any_rtc!(self, r => r.set_datetime(t).map_err(AnyError::from))
    }
    fn power_lost(&mut self) -> Result<bool, AnyError> {
        any_rtc!(self, r => r.power_lost().map_err(AnyError::from))
    }
}

impl Speaker for AnySpeaker {
    type Error = AnyError;
    fn sample_rate(&self) -> u32 {
        any_speaker!(self, s => s.sample_rate())
    }
    fn play(&mut self, frames: &[[i16; 2]]) -> Result<(), AnyError> {
        any_speaker!(self, s => s.play(frames).map_err(AnyError::from))
    }
    fn idle(&mut self) -> Result<(), AnyError> {
        any_speaker!(self, s => s.idle().map_err(AnyError::from))
    }
    fn set_volume(&mut self, level: u8) -> Result<(), AnyError> {
        any_speaker!(self, s => s.set_volume(level).map_err(AnyError::from))
    }
    fn restart(&mut self) -> Result<(), AnyError> {
        any_speaker!(self, s => s.restart().map_err(AnyError::from))
    }
}

impl Microphone for AnyMicrophone {
    type Error = AnyError;
    fn sample_rate(&self) -> u32 {
        any_microphone!(self, m => m.sample_rate())
    }
    fn read(&mut self, out: &mut [[i16; 2]]) -> Result<usize, AnyError> {
        any_microphone!(self, m => m.read(out).map_err(AnyError::from))
    }
}

impl Power for AnyPower {
    type Error = AnyError;
    fn battery_mv(&mut self) -> Result<Option<u16>, AnyError> {
        any_power!(self, p => p.battery_mv().map_err(AnyError::from))
    }
    fn vbus_present(&mut self) -> Result<Option<bool>, AnyError> {
        any_power!(self, p => p.vbus_present().map_err(AnyError::from))
    }
}
