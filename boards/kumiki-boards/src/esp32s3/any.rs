//! `Any*` enums: one type per device class across the boards of this family
//! (what kumiki-dtgen will generate for a multi-board build, doc/07 §6.3).

use embedded_graphics_core::{
    draw_target::DrawTarget,
    geometry::{OriginDimensions, Size},
    pixelcolor::Rgb565,
    primitives::Rectangle,
    Pixel,
};
use esp_hal::gpio::Output;
use kumiki_class::{Backlight, Imu, Power, Touch, TouchPoint, Vec3};
use kumiki_gfx::{FastDraw, PixelFormat, Rgb565Be};
use kumiki_gfx_bus_esp::Error as BusError;
use kumiki_mpu6886::Mpu6886;

use super::{AtomS3Display, CoreS3Display, I2cBus, SharedI2c, chips};

/// Display of any board in the family.
pub enum AnyDisplay {
    /// CoreS3: ILI9342C 320×240.
    CoreS3(CoreS3Display),
    /// AtomS3: GC9107 128×128.
    AtomS3(AtomS3Display),
}

macro_rules! each_display {
    ($self:expr, $d:ident => $e:expr) => {
        match $self {
            AnyDisplay::CoreS3($d) => $e,
            AnyDisplay::AtomS3($d) => $e,
        }
    };
}

impl OriginDimensions for AnyDisplay {
    fn size(&self) -> Size {
        each_display!(self, d => d.size())
    }
}

impl DrawTarget for AnyDisplay {
    type Color = Rgb565;
    type Error = BusError;

    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(&mut self, pixels: I) -> Result<(), BusError> {
        each_display!(self, d => d.draw_iter(pixels))
    }
    fn fill_contiguous<I: IntoIterator<Item = Rgb565>>(&mut self, area: &Rectangle, colors: I) -> Result<(), BusError> {
        each_display!(self, d => d.fill_contiguous(area, colors))
    }
    fn fill_solid(&mut self, area: &Rectangle, color: Rgb565) -> Result<(), BusError> {
        each_display!(self, d => d.fill_solid(area, color))
    }
    fn clear(&mut self, color: Rgb565) -> Result<(), BusError> {
        each_display!(self, d => d.clear(color))
    }
}

impl FastDraw for AnyDisplay {
    type Format = Rgb565Be;
    fn fill_rect(&mut self, area: &Rectangle, color: Rgb565) -> Result<(), BusError> {
        each_display!(self, d => d.fill_rect(area, color))
    }
    fn blit<S: PixelFormat>(&mut self, area: &Rectangle, src: &[u8], stride_px: usize) -> Result<(), BusError> {
        each_display!(self, d => d.blit::<S>(area, src, stride_px))
    }
}

/// Backlight of any board in the family.
pub enum AnyBacklight {
    /// AXP2101 DLDO1 (CoreS3).
    Axp2101(chips::Axp2101Backlight<SharedI2c>),
    /// Plain GPIO (AtomS3; PWM dimming to come).
    Gpio(Output<'static>),
}

/// Backlight error.
#[derive(Debug)]
pub enum AnyError {
    /// I²C failure.
    I2c(esp_hal::i2c::master::Error),
}

impl Backlight for AnyBacklight {
    type Error = AnyError;
    fn set_brightness(&mut self, level: u8) -> Result<(), AnyError> {
        match self {
            AnyBacklight::Axp2101(b) => b.set_brightness(level).map_err(AnyError::I2c),
            AnyBacklight::Gpio(pin) => {
                pin.set_level((level > 0).into());
                Ok(())
            }
        }
    }
}

/// Touch panel of any board in the family.
pub enum AnyTouch {
    /// FT6336U (CoreS3).
    Ft6336(chips::Ft6336<SharedI2c>),
}

impl Touch for AnyTouch {
    type Error = AnyError;
    fn read(&mut self, out: &mut [TouchPoint]) -> Result<usize, AnyError> {
        match self {
            AnyTouch::Ft6336(t) => t.read(out).map_err(AnyError::I2c),
        }
    }
}

/// IMU of any board in the family.
pub enum AnyImu {
    /// MPU-6886 (AtomS3).
    Mpu6886(Mpu6886<I2cBus>),
}

impl Imu for AnyImu {
    type Error = AnyError;
    fn accel(&mut self) -> Result<Vec3, AnyError> {
        match self {
            AnyImu::Mpu6886(m) => {
                let raw = m.accel_raw().map_err(|e| match e {
                    kumiki_mpu6886::Error::I2c(e) => AnyError::I2c(e),
                    kumiki_mpu6886::Error::WrongId(_) => unreachable!(),
                })?;
                let [x, y, z] = Mpu6886::<I2cBus>::accel_g(raw);
                Ok(Vec3 { x, y, z })
            }
        }
    }
    fn gyro(&mut self) -> Result<Vec3, AnyError> {
        match self {
            AnyImu::Mpu6886(m) => {
                let raw = m.gyro_raw().map_err(|e| match e {
                    kumiki_mpu6886::Error::I2c(e) => AnyError::I2c(e),
                    kumiki_mpu6886::Error::WrongId(_) => unreachable!(),
                })?;
                let [x, y, z] = Mpu6886::<I2cBus>::gyro_dps(raw);
                Ok(Vec3 { x, y, z })
            }
        }
    }
}

/// Power information of any board in the family.
pub enum AnyPower {
    /// AXP2101 (CoreS3).
    Axp2101(chips::Axp2101Power<SharedI2c>),
}

impl Power for AnyPower {
    type Error = AnyError;
    fn battery_mv(&mut self) -> Result<Option<u16>, AnyError> {
        match self {
            AnyPower::Axp2101(p) => p.battery_mv().map_err(AnyError::I2c),
        }
    }
    fn vbus_present(&mut self) -> Result<Option<bool>, AnyError> {
        match self {
            AnyPower::Axp2101(p) => p.vbus_present().map_err(AnyError::I2c),
        }
    }
}
