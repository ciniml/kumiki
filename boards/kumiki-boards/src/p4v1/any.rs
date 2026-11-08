//! `Any*` enums of the ESP32-P4 rev v1 family (one board so far: Tab5).
//! Same shape and method names as the ESP32-S3 family, so applications can
//! be written against either.

use embedded_graphics_core::{
    draw_target::DrawTarget,
    geometry::{Dimensions, OriginDimensions, Size},
    pixelcolor::Rgb565,
    primitives::Rectangle,
    Pixel,
};
use kumiki_bmi270::Bmi270;
use kumiki_class::{Backlight, Imu, Power, Touch, TouchPoint, Vec3};
use kumiki_gfx::{FastDraw, FbDisplay, PixelFormat, Rgb565Le};
use kumiki_p4v1_hal::{fb::DsiPanel, gpio::Output, i2c};

use super::{SharedI2c, chips};

/// Display of any board in the family. Call [`AnyDisplay::flush`] after
/// drawing a frame: it writes the changes back from the cache and presents
/// them (double-buffered).
pub enum AnyDisplay {
    /// ILI9881C 720×1280 over MIPI-DSI (Tab5).
    Ili9881c(FbDisplay<DsiPanel>),
}

/// Display error: a 2D-engine (PPA) failure while presenting.
pub type DisplayError = kumiki_p4v1_hal::fb::PpaError;

fn infallible<T>(r: Result<T, core::convert::Infallible>) -> Result<T, DisplayError> {
    r.map_err(|e| match e {})
}

impl OriginDimensions for AnyDisplay {
    fn size(&self) -> Size {
        match self {
            AnyDisplay::Ili9881c(d) => d.size(),
        }
    }
}

impl DrawTarget for AnyDisplay {
    type Color = Rgb565;
    type Error = DisplayError;

    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(&mut self, pixels: I) -> Result<(), DisplayError> {
        match self {
            AnyDisplay::Ili9881c(d) => infallible(d.draw_iter(pixels)),
        }
    }
    fn fill_contiguous<I: IntoIterator<Item = Rgb565>>(&mut self, area: &Rectangle, colors: I) -> Result<(), DisplayError> {
        match self {
            AnyDisplay::Ili9881c(d) => infallible(d.fill_contiguous(area, colors)),
        }
    }
    fn fill_solid(&mut self, area: &Rectangle, color: Rgb565) -> Result<(), DisplayError> {
        match self {
            AnyDisplay::Ili9881c(d) => infallible(d.fill_solid(area, color)),
        }
    }
    fn clear(&mut self, color: Rgb565) -> Result<(), DisplayError> {
        match self {
            AnyDisplay::Ili9881c(d) => infallible(d.clear(color)),
        }
    }
}

impl FastDraw for AnyDisplay {
    type Format = Rgb565Le;
    fn fill_rect(&mut self, area: &Rectangle, color: Rgb565) -> Result<(), DisplayError> {
        match self {
            AnyDisplay::Ili9881c(d) => infallible(FastDraw::fill_rect(d, area, color)),
        }
    }
    fn blit<S: PixelFormat>(&mut self, area: &Rectangle, src: &[u8], stride_px: usize) -> Result<(), DisplayError> {
        match self {
            AnyDisplay::Ili9881c(d) => infallible(d.blit::<S>(area, src, stride_px)),
        }
    }
}

impl AnyDisplay {
    /// The area guaranteed to be visible (the whole screen here).
    pub fn safe_area(&self) -> Rectangle {
        self.bounding_box()
    }

    /// Present the frame (cache write-back of the changed area, buffer swap).
    pub fn flush(&mut self) -> Result<(), DisplayError> {
        match self {
            AnyDisplay::Ili9881c(d) => d.flush(),
        }
    }

    /// Keep the scan-out going while not presenting for a while (call at
    /// least every ~200 ms; `flush` does it too).
    pub fn service(&mut self) {
        match self {
            AnyDisplay::Ili9881c(d) => d.panel_mut().service(),
        }
    }
}

/// Backlight of any board in the family.
pub enum AnyBacklight {
    /// Plain GPIO (Tab5: on / off).
    Gpio(Output),
    /// Not controllable.
    None,
}

/// Error of the `Any*` devices.
#[derive(Debug)]
pub enum AnyError {
    /// I²C failure.
    I2c(i2c::Error),
    /// The device reported an unexpected state.
    Device,
}

impl Backlight for AnyBacklight {
    type Error = AnyError;
    fn set_brightness(&mut self, level: u8) -> Result<(), AnyError> {
        match self {
            AnyBacklight::Gpio(pin) => {
                let _ = if level > 0 {
                    embedded_hal::digital::OutputPin::set_high(pin)
                } else {
                    embedded_hal::digital::OutputPin::set_low(pin)
                };
                Ok(())
            }
            AnyBacklight::None => Ok(()),
        }
    }
}

/// Touch panel of any board in the family.
pub enum AnyTouch {
    /// GT911 (Tab5, ILI9881C lot).
    Gt911(chips::Gt911<SharedI2c>),
}

impl AnyTouch {
    /// Controller name.
    pub fn name(&self) -> &'static str {
        match self {
            AnyTouch::Gt911(_) => "GT911",
        }
    }
}

impl Touch for AnyTouch {
    type Error = AnyError;
    fn read(&mut self, out: &mut [TouchPoint]) -> Result<usize, AnyError> {
        match self {
            AnyTouch::Gt911(t) => t.read(out).map_err(AnyError::I2c),
        }
    }
}

/// IMU of any board in the family.
pub enum AnyImu {
    /// BMI270 on the internal bus (Tab5).
    Bmi270(Bmi270<SharedI2c>),
}

fn bmi_err(e: kumiki_bmi270::Error<i2c::Error>) -> AnyError {
    match e {
        kumiki_bmi270::Error::I2c(e) => AnyError::I2c(e),
        _ => AnyError::Device,
    }
}

impl Imu for AnyImu {
    type Error = AnyError;
    fn accel(&mut self) -> Result<Vec3, AnyError> {
        let [x, y, z] = match self {
            AnyImu::Bmi270(m) => Bmi270::<SharedI2c>::accel_g(m.accel_raw().map_err(bmi_err)?),
        };
        Ok(Vec3 { x, y, z })
    }
    fn gyro(&mut self) -> Result<Vec3, AnyError> {
        let [x, y, z] = match self {
            AnyImu::Bmi270(m) => Bmi270::<SharedI2c>::gyro_dps(m.gyro_raw().map_err(bmi_err)?),
        };
        Ok(Vec3 { x, y, z })
    }
}

/// Power information of any board in the family (none described yet; the
/// Tab5's INA226 / charger will go here).
pub enum AnyPower {}

impl Power for AnyPower {
    type Error = AnyError;
    fn battery_mv(&mut self) -> Result<Option<u16>, AnyError> {
        match *self {}
    }
    fn vbus_present(&mut self) -> Result<Option<bool>, AnyError> {
        match *self {}
    }
}
