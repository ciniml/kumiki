//! Trait impls of the `Any*` enums. The enums themselves (one variant per
//! device the family's device trees use) and their dispatch macros
//! (`any_touch!` …) are generated into [`super::generated`] from the
//! bindings' `kumiki.variant` / `kumiki.type`; the impls below are written
//! once per class, so adding a device to a DTS needs no edit here.

use embedded_graphics_core::{
    draw_target::DrawTarget,
    geometry::{Dimensions, OriginDimensions, Point, Size},
    pixelcolor::Rgb565,
    primitives::Rectangle,
    Pixel,
};
use kumiki_class::{Backlight, DateTime, Imu, Power, Rtc, Speaker, Touch, TouchPoint, Vec3};
use kumiki_gfx::{AsyncFastDraw, FastDraw, PixelFormat, Rgb565Be};
use kumiki_gfx_bus_esp::Error as BusError;

use super::{
    AnyBacklight, AnyDisplay, AnyImu, AnyPower, AnyRtc, AnySpeaker, AnyTouch, Co5300AsyncDisplay, Gc9107AsyncDisplay, Ili9342cAsyncDisplay,
};

type I2cError = esp_hal::i2c::master::Error;

/// Error of the `Any*` devices.
#[derive(Debug)]
pub enum AnyError {
    /// I²C failure.
    I2c(I2cError),
    /// The device reported an unexpected state.
    Device,
}

impl From<I2cError> for AnyError {
    fn from(e: I2cError) -> Self {
        AnyError::I2c(e)
    }
}

impl From<core::convert::Infallible> for AnyError {
    fn from(e: core::convert::Infallible) -> Self {
        match e {}
    }
}

impl From<kumiki_mpu6886::Error<I2cError>> for AnyError {
    fn from(e: kumiki_mpu6886::Error<I2cError>) -> Self {
        match e {
            kumiki_mpu6886::Error::I2c(e) => AnyError::I2c(e),
            kumiki_mpu6886::Error::WrongId(_) => AnyError::Device,
        }
    }
}

impl From<kumiki_bmi270::Error<I2cError>> for AnyError {
    fn from(e: kumiki_bmi270::Error<I2cError>) -> Self {
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
    type Error = BusError;

    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(&mut self, pixels: I) -> Result<(), BusError> {
        any_display!(self, d => d.draw_iter(pixels).map_err(Into::into))
    }
    fn fill_contiguous<I: IntoIterator<Item = Rgb565>>(&mut self, area: &Rectangle, colors: I) -> Result<(), BusError> {
        any_display!(self, d => d.fill_contiguous(area, colors).map_err(Into::into))
    }
    fn fill_solid(&mut self, area: &Rectangle, color: Rgb565) -> Result<(), BusError> {
        any_display!(self, d => d.fill_solid(area, color).map_err(Into::into))
    }
    fn clear(&mut self, color: Rgb565) -> Result<(), BusError> {
        any_display!(self, d => d.clear(color).map_err(Into::into))
    }
}

impl FastDraw for AnyDisplay {
    type Format = Rgb565Be;
    fn fill_rect(&mut self, area: &Rectangle, color: Rgb565) -> Result<(), BusError> {
        any_display!(self, d => FastDraw::fill_rect(d, area, color).map_err(Into::into))
    }
    fn blit<S: PixelFormat>(&mut self, area: &Rectangle, src: &[u8], stride_px: usize) -> Result<(), BusError> {
        any_display!(self, d => d.blit::<S>(area, src, stride_px).map_err(Into::into))
    }
}

impl AnyDisplay {
    /// The area guaranteed to be visible. For rectangular panels this is the
    /// whole screen; for StopWatch's round panel it is the even-aligned square
    /// inscribed in the visible circle (Ø466 centred in the 468×468 frame).
    /// Draw rectangular layouts through `kumiki_gfx::Viewport::new(&mut display, safe_area)`.
    pub fn safe_area(&self) -> Rectangle {
        match self {
            AnyDisplay::Co5300(d) => {
                let s = d.size();
                let (cx, cy) = (s.width as i32 / 2, s.height as i32 / 2);
                // DT `kumiki,round-diameter`; side = d / √2, rounded down to even.
                let diameter = super::generated::stopwatch::lcd::ROUND_DIAMETER.unwrap_or(s.width) as i32;
                let side = diameter * 1000 / 1414 & !1;
                Rectangle::new(Point::new((cx - side / 2) & !1, cy - side / 2), Size::new(side as u32, side as u32))
            }
            d => d.bounding_box(),
        }
    }

    /// A touch report in screen coordinates. The ESP32-S3 boards run their
    /// panels in the native orientation, where touch and screen agree.
    pub fn touch_to_screen(&self, p: &TouchPoint) -> Point {
        Point::new(p.x as i32, p.y as i32)
    }

    /// Send pending changes (frame-buffered displays); no-op for direct ones.
    pub fn flush(&mut self) -> Result<(), BusError> {
        match self {
            AnyDisplay::Co5300(d) => d.flush(),
            _ => Ok(()),
        }
    }

    /// Switch the display to `async` operation (DMA completion via interrupt +
    /// waker, executor-agnostic). Draw into a [`kumiki_gfx::Sprite`] and push it
    /// with [`kumiki_gfx::AsyncFastDraw`]. For StopWatch the frame buffer is
    /// released; push even-aligned areas (e.g. whole frames).
    pub fn into_async(self) -> AnyAsyncDisplay {
        match self {
            AnyDisplay::Ili9342c(d) => AnyAsyncDisplay::Ili9342c(d.map_panel(|p| p.map_bus(|b| b.into_async()))),
            AnyDisplay::Gc9107(d) => AnyAsyncDisplay::Gc9107(d.map_panel(|p| p.map_bus(|b| b.into_async()))),
            AnyDisplay::Co5300(d) => {
                AnyAsyncDisplay::Co5300(d.into_target().map_panel(|p| p.map_bus(|b| b.into_async())))
            }
        }
    }
}

/// `async` display of any board in the family.
pub enum AnyAsyncDisplay {
    /// ILI9342C 320×240 (CoreS3).
    Ili9342c(Ili9342cAsyncDisplay),
    /// GC9107 128×128 (AtomS3).
    Gc9107(Gc9107AsyncDisplay),
    /// CO5300 (StopWatch; x / width must be even).
    Co5300(Co5300AsyncDisplay),
}

impl AsyncFastDraw for AnyAsyncDisplay {
    type Format = Rgb565Be;
    type Error = BusError;

    fn size(&self) -> Size {
        match self {
            AnyAsyncDisplay::Ili9342c(d) => AsyncFastDraw::size(d),
            AnyAsyncDisplay::Gc9107(d) => AsyncFastDraw::size(d),
            AnyAsyncDisplay::Co5300(d) => AsyncFastDraw::size(d),
        }
    }

    async fn fill_rect(&mut self, area: &Rectangle, color: Rgb565) -> Result<(), BusError> {
        match self {
            AnyAsyncDisplay::Ili9342c(d) => AsyncFastDraw::fill_rect(d, area, color).await,
            AnyAsyncDisplay::Gc9107(d) => AsyncFastDraw::fill_rect(d, area, color).await,
            AnyAsyncDisplay::Co5300(d) => AsyncFastDraw::fill_rect(d, area, color).await,
        }
    }

    async fn blit<S: PixelFormat>(&mut self, area: &Rectangle, src: &[u8], stride_px: usize) -> Result<(), BusError> {
        match self {
            AnyAsyncDisplay::Ili9342c(d) => AsyncFastDraw::blit::<S>(d, area, src, stride_px).await,
            AnyAsyncDisplay::Gc9107(d) => AsyncFastDraw::blit::<S>(d, area, src, stride_px).await,
            AnyAsyncDisplay::Co5300(d) => AsyncFastDraw::blit::<S>(d, area, src, stride_px).await,
        }
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
