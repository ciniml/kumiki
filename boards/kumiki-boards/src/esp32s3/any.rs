//! `Any*` enums: one type per device class across the boards of this family
//! (what kumiki-dtgen will generate for a multi-board build, doc/07 §6.3).

use embedded_graphics_core::{
    draw_target::DrawTarget,
    geometry::{Dimensions, OriginDimensions, Size},
    pixelcolor::Rgb565,
    primitives::Rectangle,
    Pixel,
};
use esp_hal::gpio::Output;
use kumiki_class::{Backlight, Imu, Power, Touch, TouchPoint, Vec3};
use kumiki_gfx::{AsyncFastDraw, FastDraw, PixelFormat, Rgb565Be};
use kumiki_gfx_bus_esp::Error as BusError;
use kumiki_bmi270::Bmi270;
use kumiki_mpu6886::Mpu6886;

use super::{
    AtomS3AsyncDisplay, AtomS3Display, CoreS3AsyncDisplay, CoreS3Display, I2cBus, SharedI2c, StopWatchAsyncDisplay,
    StopWatchDisplay, chips,
};

/// Display of any board in the family.
///
/// Call [`AnyDisplay::flush`] after drawing a frame: frame-buffered displays
/// (StopWatch) send the changed area then; direct ones do nothing.
pub enum AnyDisplay {
    /// CoreS3: ILI9342C 320×240.
    CoreS3(CoreS3Display),
    /// AtomS3: GC9107 128×128.
    AtomS3(AtomS3Display),
    /// StopWatch: CO5300 468×468 (466 visible, round), frame-buffered in PSRAM.
    StopWatch(StopWatchDisplay),
}

/// Drawing into a RAM frame buffer cannot fail.
fn infallible<T>(r: Result<T, core::convert::Infallible>) -> Result<T, BusError> {
    r.map_err(|e| match e {})
}

impl OriginDimensions for AnyDisplay {
    fn size(&self) -> Size {
        match self {
            AnyDisplay::CoreS3(d) => d.size(),
            AnyDisplay::AtomS3(d) => d.size(),
            AnyDisplay::StopWatch(d) => d.size(),
        }
    }
}

impl DrawTarget for AnyDisplay {
    type Color = Rgb565;
    type Error = BusError;

    fn draw_iter<I: IntoIterator<Item = Pixel<Rgb565>>>(&mut self, pixels: I) -> Result<(), BusError> {
        match self {
            AnyDisplay::CoreS3(d) => d.draw_iter(pixels),
            AnyDisplay::AtomS3(d) => d.draw_iter(pixels),
            AnyDisplay::StopWatch(d) => infallible(d.draw_iter(pixels)),
        }
    }
    fn fill_contiguous<I: IntoIterator<Item = Rgb565>>(&mut self, area: &Rectangle, colors: I) -> Result<(), BusError> {
        match self {
            AnyDisplay::CoreS3(d) => d.fill_contiguous(area, colors),
            AnyDisplay::AtomS3(d) => d.fill_contiguous(area, colors),
            AnyDisplay::StopWatch(d) => infallible(d.fill_contiguous(area, colors)),
        }
    }
    fn fill_solid(&mut self, area: &Rectangle, color: Rgb565) -> Result<(), BusError> {
        match self {
            AnyDisplay::CoreS3(d) => d.fill_solid(area, color),
            AnyDisplay::AtomS3(d) => d.fill_solid(area, color),
            AnyDisplay::StopWatch(d) => infallible(d.fill_solid(area, color)),
        }
    }
    fn clear(&mut self, color: Rgb565) -> Result<(), BusError> {
        match self {
            AnyDisplay::CoreS3(d) => d.clear(color),
            AnyDisplay::AtomS3(d) => d.clear(color),
            AnyDisplay::StopWatch(d) => infallible(d.clear(color)),
        }
    }
}

impl FastDraw for AnyDisplay {
    type Format = Rgb565Be;
    fn fill_rect(&mut self, area: &Rectangle, color: Rgb565) -> Result<(), BusError> {
        match self {
            AnyDisplay::CoreS3(d) => d.fill_rect(area, color),
            AnyDisplay::AtomS3(d) => d.fill_rect(area, color),
            AnyDisplay::StopWatch(d) => infallible(d.fill_rect(area, color)),
        }
    }
    fn blit<S: PixelFormat>(&mut self, area: &Rectangle, src: &[u8], stride_px: usize) -> Result<(), BusError> {
        match self {
            AnyDisplay::CoreS3(d) => d.blit::<S>(area, src, stride_px),
            AnyDisplay::AtomS3(d) => d.blit::<S>(area, src, stride_px),
            AnyDisplay::StopWatch(d) => infallible(d.blit::<S>(area, src, stride_px)),
        }
    }
}

impl AnyDisplay {
    /// The area guaranteed to be visible. For rectangular panels this is the
    /// whole screen; for StopWatch's round panel it is the even-aligned square
    /// inscribed in the visible circle (Ø466 centred in the 468×468 frame).
    /// Draw rectangular layouts through `kumiki_gfx::Viewport::new(&mut display, safe_area)`.
    pub fn safe_area(&self) -> Rectangle {
        match self {
            AnyDisplay::StopWatch(d) => {
                let s = d.size();
                let (cx, cy) = (s.width as i32 / 2, s.height as i32 / 2);
                // side = d / √2 for d = 466 → 329.5, rounded down to even.
                let side = 466 * 1000 / 1414 & !1;
                Rectangle::new(
                    embedded_graphics_core::geometry::Point::new((cx - side / 2) & !1, cy - side / 2),
                    Size::new(side as u32, side as u32),
                )
            }
            d => d.bounding_box(),
        }
    }

    /// Send pending changes (frame-buffered displays); no-op for direct ones.
    pub fn flush(&mut self) -> Result<(), BusError> {
        match self {
            AnyDisplay::StopWatch(d) => d.flush(),
            _ => Ok(()),
        }
    }

    /// Switch the display to `async` operation (DMA completion via interrupt +
    /// waker, executor-agnostic). Draw into a [`kumiki_gfx::Sprite`] and push it
    /// with [`kumiki_gfx::AsyncFastDraw`]. For StopWatch the frame buffer is
    /// released; push even-aligned areas (e.g. whole frames).
    pub fn into_async(self) -> AnyAsyncDisplay {
        match self {
            AnyDisplay::CoreS3(d) => AnyAsyncDisplay::CoreS3(d.map_panel(|p| p.map_bus(|b| b.into_async()))),
            AnyDisplay::AtomS3(d) => AnyAsyncDisplay::AtomS3(d.map_panel(|p| p.map_bus(|b| b.into_async()))),
            AnyDisplay::StopWatch(d) => {
                AnyAsyncDisplay::StopWatch(d.into_target().map_panel(|p| p.map_bus(|b| b.into_async())))
            }
        }
    }
}

/// `async` display of any board in the family.
pub enum AnyAsyncDisplay {
    /// CoreS3: ILI9342C 320×240.
    CoreS3(CoreS3AsyncDisplay),
    /// AtomS3: GC9107 128×128.
    AtomS3(AtomS3AsyncDisplay),
    /// StopWatch: CO5300 468×468 (x / width must be even).
    StopWatch(StopWatchAsyncDisplay),
}

impl AsyncFastDraw for AnyAsyncDisplay {
    type Format = Rgb565Be;
    type Error = BusError;

    fn size(&self) -> Size {
        match self {
            AnyAsyncDisplay::CoreS3(d) => AsyncFastDraw::size(d),
            AnyAsyncDisplay::AtomS3(d) => AsyncFastDraw::size(d),
            AnyAsyncDisplay::StopWatch(d) => AsyncFastDraw::size(d),
        }
    }

    async fn fill_rect(&mut self, area: &Rectangle, color: Rgb565) -> Result<(), BusError> {
        match self {
            AnyAsyncDisplay::CoreS3(d) => AsyncFastDraw::fill_rect(d, area, color).await,
            AnyAsyncDisplay::AtomS3(d) => AsyncFastDraw::fill_rect(d, area, color).await,
            AnyAsyncDisplay::StopWatch(d) => AsyncFastDraw::fill_rect(d, area, color).await,
        }
    }

    async fn blit<S: PixelFormat>(&mut self, area: &Rectangle, src: &[u8], stride_px: usize) -> Result<(), BusError> {
        match self {
            AnyAsyncDisplay::CoreS3(d) => AsyncFastDraw::blit::<S>(d, area, src, stride_px).await,
            AnyAsyncDisplay::AtomS3(d) => AsyncFastDraw::blit::<S>(d, area, src, stride_px).await,
            AnyAsyncDisplay::StopWatch(d) => AsyncFastDraw::blit::<S>(d, area, src, stride_px).await,
        }
    }
}

/// Backlight of any board in the family.
pub enum AnyBacklight {
    /// AXP2101 DLDO1 (CoreS3).
    Axp2101(chips::Axp2101Backlight<SharedI2c>),
    /// Plain GPIO (AtomS3; PWM dimming to come).
    Gpio(Output<'static>),
    /// Not controllable yet (StopWatch: AMOLED brightness is a panel command, set at init).
    None,
}

/// Error of the `Any*` devices.
#[derive(Debug)]
pub enum AnyError {
    /// I²C failure.
    I2c(esp_hal::i2c::master::Error),
    /// The device reported an unexpected state.
    Device,
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
            AnyBacklight::None => Ok(()),
        }
    }
}

/// Touch panel of any board in the family.
pub enum AnyTouch {
    /// FT6336U (CoreS3).
    Ft6336(chips::Ft6336<SharedI2c>),
    /// CST820 (StopWatch).
    Cst820(chips::Cst820<SharedI2c>),
    /// CST9217 (StopWatch, two-finger variant).
    Cst9217(chips::Cst9217<SharedI2c>),
}

impl AnyTouch {
    /// Controller name.
    pub fn name(&self) -> &'static str {
        match self {
            AnyTouch::Ft6336(_) => "FT6336U",
            AnyTouch::Cst820(_) => "CST820",
            AnyTouch::Cst9217(_) => "CST9217",
        }
    }
}

impl Touch for AnyTouch {
    type Error = AnyError;
    fn read(&mut self, out: &mut [TouchPoint]) -> Result<usize, AnyError> {
        match self {
            AnyTouch::Ft6336(t) => t.read(out).map_err(AnyError::I2c),
            AnyTouch::Cst820(t) => t.read(out).map_err(AnyError::I2c),
            AnyTouch::Cst9217(t) => t.read(out).map_err(AnyError::I2c),
        }
    }
}

/// IMU of any board in the family.
pub enum AnyImu {
    /// MPU-6886 (AtomS3).
    Mpu6886(Mpu6886<I2cBus>),
    /// BMI270 on the shared internal bus (CoreS3).
    Bmi270(Bmi270<SharedI2c>),
}

fn mpu_err(e: kumiki_mpu6886::Error<esp_hal::i2c::master::Error>) -> AnyError {
    match e {
        kumiki_mpu6886::Error::I2c(e) => AnyError::I2c(e),
        kumiki_mpu6886::Error::WrongId(_) => AnyError::Device,
    }
}

fn bmi_err(e: kumiki_bmi270::Error<esp_hal::i2c::master::Error>) -> AnyError {
    match e {
        kumiki_bmi270::Error::I2c(e) => AnyError::I2c(e),
        _ => AnyError::Device,
    }
}

impl Imu for AnyImu {
    type Error = AnyError;
    fn accel(&mut self) -> Result<Vec3, AnyError> {
        let [x, y, z] = match self {
            AnyImu::Mpu6886(m) => Mpu6886::<I2cBus>::accel_g(m.accel_raw().map_err(mpu_err)?),
            AnyImu::Bmi270(m) => Bmi270::<SharedI2c>::accel_g(m.accel_raw().map_err(bmi_err)?),
        };
        Ok(Vec3 { x, y, z })
    }
    fn gyro(&mut self) -> Result<Vec3, AnyError> {
        let [x, y, z] = match self {
            AnyImu::Mpu6886(m) => Mpu6886::<I2cBus>::gyro_dps(m.gyro_raw().map_err(mpu_err)?),
            AnyImu::Bmi270(m) => Bmi270::<SharedI2c>::gyro_dps(m.gyro_raw().map_err(bmi_err)?),
        };
        Ok(Vec3 { x, y, z })
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
