//! ESP32-S3 board family: M5Stack CoreS3, AtomS3 and StopWatch.
//!
//! Board data comes from the device tree: `boards/dts/m5stack/*.dts` is
//! compiled by `kumiki-dtgen` into [`generated`] (board ids, detection,
//! pin bundles, per-device constants, init order). This file holds the
//! bring-up sequences that consume that data; generating them from binding
//! templates is the next step (doc/11 M3).
//!
//! Detection (generated, see [`generated::detect`]): candidates whose pins
//! clash with embedded octal PSRAM (eFuse `PSRAM_CAP`, GPIO33..37) are skipped
//! on such chips, and candidates that need PSRAM are skipped without it.
//!
//! Regenerate after editing a DTS or binding:
//! `cargo run -p kumiki-dtgen -- esp32s3 boards/kumiki-boards/src/esp32s3/generated.rs boards/dts/m5stack/cores3.dts boards/dts/m5stack/atoms3.dts boards/dts/m5stack/stopwatch.dts`

#[macro_use]
pub mod generated;
mod any;
pub mod chips;
pub mod probe;

use core::cell::RefCell;

use embedded_hal_bus::i2c::RefCellDevice;
use esp_hal::{
    Blocking,
    delay::Delay,
    dma::DmaTxBuf,
    dma_tx_buffer,
    gpio::{Input, InputConfig, Level, Output, OutputConfig, Pull},
    i2c::master::{Config as I2cConfig, I2c},
    peripherals::Peripherals,
    spi::{
        Mode,
        master::{Config as SpiConfig, Spi},
    },
    time::Rate,
};
use kumiki_class::{ButtonId, TouchPoint};
use kumiki_gfx::Display;
use kumiki_gfx_bus_esp::{EspQspiDmaBus, EspSpiDmaBus, SpiHost};
use kumiki_gfx_panels::{Co5300, DcsConfig, DcsPanel, Gc9107, Ili9342c, Model, NoPin};
use kumiki_mpu6886::Mpu6886;
use static_cell::StaticCell;

pub use any::{AnyAsyncDisplay, AnyBacklight, AnyDisplay, AnyImu, AnyPower, AnyTouch};
pub use generated::{BoardId, DetectLog};

/// Blocking I²C master.
pub type I2cBus = I2c<'static, Blocking>;
/// A device handle on a shared (single-core) I²C bus.
pub type SharedI2c = RefCellDevice<'static, I2cBus>;
/// Panel bus used by the SPI displays of this family.
pub type PanelBus = EspSpiDmaBus<'static, Blocking>;
/// CoreS3 display.
pub type CoreS3Display = Display<DcsPanel<PanelBus, Ili9342c>>;
/// AtomS3 display.
pub type AtomS3Display = Display<DcsPanel<PanelBus, Gc9107>>;
/// `async` panel bus.
pub type AsyncPanelBus = EspSpiDmaBus<'static, esp_hal::Async>;
/// CoreS3 display, `async`.
pub type CoreS3AsyncDisplay = Display<DcsPanel<AsyncPanelBus, Ili9342c>>;
/// AtomS3 display, `async`.
pub type AtomS3AsyncDisplay = Display<DcsPanel<AsyncPanelBus, Gc9107>>;
/// StopWatch display (CO5300 over QSPI, frame-buffered in PSRAM).
pub type StopWatchDisplay = kumiki_gfx::BufferedDisplay<
    Display<DcsPanel<EspQspiDmaBus<'static, Blocking>, Co5300>>,
    kumiki_gfx::Rgb565Be,
    &'static mut [u8],
>;
/// StopWatch display, `async` (push whole aligned frames from your own buffer).
pub type StopWatchAsyncDisplay = Display<DcsPanel<EspQspiDmaBus<'static, esp_hal::Async>, Co5300>>;

/// Bring-up error.
#[derive(Debug)]
pub enum InitError {
    /// No candidate board matched; what each probe read.
    UnknownBoard(DetectLog),
    /// A peripheral could not be configured.
    Config,
    /// I²C failure during bring-up.
    I2c(esp_hal::i2c::master::Error),
    /// Panel initialization failed.
    Display,
}

/// Raw button sources of the detected board.
pub enum ButtonInputs {
    /// Active-low GPIO buttons A and (optionally) B.
    Gpio {
        /// Button A.
        a: Input<'static>,
        /// Button B.
        b: Option<Input<'static>>,
    },
    /// No front buttons: a touch anywhere acts as button A (CoreS3).
    TouchAsA,
}

/// Devices of the detected board. Every field can be moved out and owned separately.
pub struct Devices {
    /// Which board was detected.
    pub board: BoardId,
    /// Display (embedded-graphics `DrawTarget` + `FastDraw`).
    pub display: AnyDisplay,
    /// Backlight.
    pub backlight: AnyBacklight,
    /// Touch panel, if any.
    pub touch: Option<AnyTouch>,
    /// IMU, if present.
    pub imu: Option<AnyImu>,
    /// Power / battery information, if any.
    pub power: Option<AnyPower>,
    /// Button raw-state source.
    pub buttons: ButtonInputs,
    /// Peripherals the board definition does not use, handed back to the
    /// application (timers and software interrupts for an RTOS/executor,
    /// radio, RNG, second core, ...).
    pub spare: Spare,
}

/// Peripherals not used by any board of this family.
#[allow(missing_docs)]
pub struct Spare {
    pub timg0: esp_hal::peripherals::TIMG0<'static>,
    pub timg1: esp_hal::peripherals::TIMG1<'static>,
    pub systimer: esp_hal::peripherals::SYSTIMER<'static>,
    pub from_cpu_intr0: esp_hal::peripherals::FROM_CPU_INTR0<'static>,
    pub from_cpu_intr1: esp_hal::peripherals::FROM_CPU_INTR1<'static>,
    pub from_cpu_intr2: esp_hal::peripherals::FROM_CPU_INTR2<'static>,
    pub from_cpu_intr3: esp_hal::peripherals::FROM_CPU_INTR3<'static>,
    pub wifi: esp_hal::peripherals::WIFI<'static>,
    pub bt: esp_hal::peripherals::BT<'static>,
    pub rng: esp_hal::peripherals::RNG<'static>,
    pub cpu_ctrl: esp_hal::peripherals::CPU_CTRL<'static>,
    pub usb_device: esp_hal::peripherals::USB_DEVICE<'static>,
    pub lpwr: esp_hal::peripherals::LPWR<'static>,
}

macro_rules! take_spare {
    ($p:ident) => {
        Spare {
            timg0: $p.TIMG0,
            timg1: $p.TIMG1,
            systimer: $p.SYSTIMER,
            from_cpu_intr0: $p.FROM_CPU_INTR0,
            from_cpu_intr1: $p.FROM_CPU_INTR1,
            from_cpu_intr2: $p.FROM_CPU_INTR2,
            from_cpu_intr3: $p.FROM_CPU_INTR3,
            wifi: $p.WIFI,
            bt: $p.BT,
            rng: $p.RNG,
            cpu_ctrl: $p.CPU_CTRL,
            usb_device: $p.USB_DEVICE,
            lpwr: $p.LPWR,
        }
    };
}

impl ButtonInputs {
    /// Raw state bits for [`kumiki_class::Buttons::update`]. `touch` is used
    /// by boards that map touches to buttons.
    pub fn raw(&mut self, touch: Option<&mut AnyTouch>) -> u8 {
        match self {
            ButtonInputs::Gpio { a, b } => {
                ((a.is_low() as u8) << ButtonId::A as u8)
                    | ((b.as_ref().is_some_and(|b| b.is_low()) as u8) << ButtonId::B as u8)
            }
            ButtonInputs::TouchAsA => {
                let mut pts = [TouchPoint::default(); 1];
                let touched = touch
                    .map(|t| kumiki_class::Touch::read(t, &mut pts).map(|n| n > 0).unwrap_or(false))
                    .unwrap_or(false);
                (touched as u8) << ButtonId::A as u8
            }
        }
    }
}

static I2C_INT: StaticCell<RefCell<I2cBus>> = StaticCell::new();

/// DcsConfig from generated panel data.
fn dcs_config(width: u32, height: u32, offset: &[u32], invert: bool, rgb_order: Option<&str>) -> DcsConfig {
    DcsConfig {
        panel_size: (width as u16, height as u16),
        offset: (offset.first().copied().unwrap_or(0) as u16, offset.get(1).copied().unwrap_or(0) as u16),
        offset_rotation: 0,
        invert,
        rgb_order: rgb_order == Some("rgb"),
    }
}

fn hz(v: Option<u32>, default: u32) -> Rate {
    Rate::from_hz(v.unwrap_or(default))
}

/// Detect the board and bring every device up. Call once, after `esp_hal::init`.
///
/// Allocate large buffers (frame buffers, sprites) before starting Wi-Fi.
pub fn init(mut p: Peripherals) -> Result<Devices, InitError> {
    // PSRAM_CAP: 0 = none, 2 = 2 MB quad; other values are embedded octal PSRAM.
    let cap: u8 = esp_hal::efuse::read_field_le(esp_hal::efuse::PSRAM_CAP);
    let (psram, octal_psram) = (cap != 0, cap != 0 && cap != 2);
    let board = generated::detect(&mut p, octal_psram, psram).map_err(InitError::UnknownBoard)?;
    // One pair of DMA buffers for whichever display the board has.
    let buf_a = dma_tx_buffer!(16384).map_err(|_| InitError::Config)?;
    let buf_b = dma_tx_buffer!(16384).map_err(|_| InitError::Config)?;
    match board {
        BoardId::M5StackCoreS3 => init_cores3(p, buf_a, buf_b),
        BoardId::M5StackAtomS3 => init_atoms3(p, buf_a, buf_b),
        BoardId::M5StackStopWatch => init_stopwatch(p, buf_a, buf_b),
    }
}

#[inline(never)]
fn init_cores3(p: Peripherals, buf_a: DmaTxBuf, buf_b: DmaTxBuf) -> Result<Devices, InitError> {
    use generated::cores3::*;
    let pins = take_pins_cores3!(p);
    let spare = take_spare!(p);
    let mut delay = Delay::new();
    // INIT_ORDER: i2c0 → pmic → ioexp → touch → imu → rtc → spi2 → lcd → lcd_backlight → buttons
    let i2c = I2c::new(p.I2C0, I2cConfig::default().with_frequency(hz(i2c0::CLOCK_FREQUENCY, 100_000)))
        .map_err(|_| InitError::Config)?
        .with_sda(pins.i2c0_sda)
        .with_scl(pins.i2c0_scl);
    let bus: &'static RefCell<I2cBus> = I2C_INT.init(RefCell::new(i2c));
    {
        let mut dev = RefCellDevice::new(bus);
        // lcd.reset-gpios = <&ioexp 13 ACTIVE_LOW> (AW9523B P1_5); lcd.power-supply = <&aldo3>
        chips::aw9523_init_cores3(&mut dev, false).map_err(InitError::I2c)?;
        chips::axp2101_power_on_cores3(&mut dev).map_err(InitError::I2c)?;
        delay.delay_millis(10);
        chips::aw9523_init_cores3(&mut dev, true).map_err(InitError::I2c)?;
        delay.delay_millis(120);
    }

    let spi = Spi::new(p.SPI2, SpiConfig::default().with_frequency(Rate::from_hz(lcd::SPI_MAX_FREQUENCY)).with_mode(Mode::_0))
        .map_err(|_| InitError::Config)?
        .with_sck(pins.spi2_sck)
        .with_mosi(pins.spi2_mosi)
        .with_dma(p.DMA_CH0);
    let dc = Output::new(pins.lcd_dc, Level::High, OutputConfig::default());
    let cs = Output::new(pins.spi2_cs, Level::High, OutputConfig::default());
    let panel_bus = EspSpiDmaBus::new(spi, dc, Some(cs), buf_a, buf_b).with_fifo(SpiHost::Spi2);
    let cfg = dcs_config(lcd::WIDTH, lcd::HEIGHT, lcd::OFFSET, lcd::INVERT, lcd::RGB_ORDER);
    let mut panel = DcsPanel::new(panel_bus, Ili9342c, cfg, None::<NoPin>);
    panel.init(&mut delay).map_err(|_| InitError::Display)?;
    let mut display = Display::new(panel);
    embedded_graphics_core::draw_target::DrawTarget::clear(&mut display, embedded_graphics_core::pixelcolor::Rgb565::new(0, 0, 0))
        .map_err(|_| InitError::Display)?;

    let mut backlight = AnyBacklight::Axp2101(chips::Axp2101Backlight(RefCellDevice::new(bus)));
    let level = lcd_backlight::DEFAULT_BRIGHTNESS.unwrap_or(160).min(255) as u8;
    kumiki_class::Backlight::set_brightness(&mut backlight, level).map_err(|_| InitError::Display)?;

    let mut bmi = kumiki_bmi270::Bmi270::new(RefCellDevice::new(bus), imu::ADDR);
    let imu = bmi.init(&mut delay).ok().map(|_| AnyImu::Bmi270(bmi));

    let touch = Some(AnyTouch::Ft6336(chips::Ft6336 { i2c: RefCellDevice::new(bus) }));
    let power = Some(AnyPower::Axp2101(chips::Axp2101Power(RefCellDevice::new(bus))));

    Ok(Devices {
        board: BoardId::M5StackCoreS3,
        display: AnyDisplay::CoreS3(display),
        backlight,
        touch,
        imu,
        power,
        buttons: ButtonInputs::TouchAsA,
        spare,
    })
}

#[inline(never)]
fn init_atoms3(p: Peripherals, buf_a: DmaTxBuf, buf_b: DmaTxBuf) -> Result<Devices, InitError> {
    use generated::atoms3::*;
    let mut pins = take_pins_atoms3!(p);
    let spare = take_spare!(p);
    let mut delay = Delay::new();
    // INIT_ORDER: i2c1 → imu → spi2 → lcd → lcd_backlight → buttons (the display goes first: the IMU is optional)
    let mut rst = Output::new(pins.lcd_reset, Level::Low, OutputConfig::default());
    delay.delay_millis(10);
    rst.set_high();
    delay.delay_millis(120);

    let spi = Spi::new(p.SPI2, SpiConfig::default().with_frequency(Rate::from_hz(lcd::SPI_MAX_FREQUENCY)).with_mode(Mode::_0))
        .map_err(|_| InitError::Config)?
        .with_sck(pins.spi2_sck)
        .with_mosi(pins.spi2_mosi)
        .with_dma(p.DMA_CH0);
    let dc = Output::new(pins.lcd_dc, Level::High, OutputConfig::default());
    let cs = Output::new(pins.spi2_cs, Level::High, OutputConfig::default());
    let panel_bus = EspSpiDmaBus::new(spi, dc, Some(cs), buf_a, buf_b).with_fifo(SpiHost::Spi2);
    let cfg = dcs_config(lcd::WIDTH, lcd::HEIGHT, lcd::OFFSET, lcd::INVERT, lcd::RGB_ORDER);
    let mut panel = DcsPanel::new(panel_bus, Gc9107, cfg, None::<NoPin>);
    panel.init(&mut delay).map_err(|_| InitError::Display)?;
    let mut display = Display::new(panel);
    embedded_graphics_core::draw_target::DrawTarget::clear(&mut display, embedded_graphics_core::pixelcolor::Rgb565::new(0, 0, 0))
        .map_err(|_| InitError::Display)?;
    // Keep the reset line released for the lifetime of the program.
    core::mem::forget(rst);

    let backlight = AnyBacklight::Gpio(Output::new(pins.lcd_backlight, Level::High, OutputConfig::default()));

    if i2c1::BUS_RECOVERY {
        probe::i2c_bus_recover(pins.i2c1_scl.reborrow(), pins.i2c1_sda.reborrow());
    }
    let i2c = I2c::new(p.I2C1, I2cConfig::default().with_frequency(hz(i2c1::CLOCK_FREQUENCY, 100_000)))
        .map_err(|_| InitError::Config)?
        .with_sda(pins.i2c1_sda)
        .with_scl(pins.i2c1_scl);
    let mut mpu = Mpu6886::new(i2c, imu::ADDR);
    let imu = mpu.init(&mut delay).ok().map(|_| AnyImu::Mpu6886(mpu));

    let button = Input::new(pins.buttons_button_a, InputConfig::default().with_pull(Pull::Up));

    Ok(Devices {
        board: BoardId::M5StackAtomS3,
        display: AnyDisplay::AtomS3(display),
        backlight,
        touch: None,
        imu,
        power: None,
        buttons: ButtonInputs::Gpio { a: button, b: None },
        spare,
    })
}

#[inline(never)]
fn init_stopwatch(p: Peripherals, buf_a: DmaTxBuf, buf_b: DmaTxBuf) -> Result<Devices, InitError> {
    use generated::stopwatch::*;
    let mut pins = take_pins_stopwatch!(p);
    let spare = take_spare!(p);
    let mut delay = Delay::new();
    let (w, h) = (lcd::WIDTH as u16, lcd::HEIGHT as u16);

    // lcd.kumiki,frame-buffer = "psram": frame buffer in octal PSRAM.
    let psram = esp_hal::psram::Psram::new(p.PSRAM, esp_hal::psram::PsramConfig::default());
    let (ptr, len) = psram.raw_parts();
    core::mem::forget(psram); // keep the mapping for the program's lifetime
    let fb_len = w as usize * h as usize * 2;
    if len < fb_len {
        return Err(InitError::Config);
    }
    // SAFETY: PSRAM is mapped for the program's lifetime and used only here.
    let fb: &'static mut [u8] = unsafe { core::slice::from_raw_parts_mut(ptr, fb_len) };

    // INIT_ORDER: i2c1 → pm1 → ioe1 → touch_* → imu → spi2 → lcd → lcd_backlight → buttons
    if i2c1::BUS_RECOVERY {
        probe::i2c_bus_recover(pins.i2c1_scl.reborrow(), pins.i2c1_sda.reborrow());
    }
    let i2c = I2c::new(p.I2C1, I2cConfig::default().with_frequency(hz(i2c1::CLOCK_FREQUENCY, 100_000)))
        .map_err(|_| InitError::Config)?
        .with_sda(pins.i2c1_sda)
        .with_scl(pins.i2c1_scl);
    let bus: &'static RefCell<I2cBus> = I2C_INT.init(RefCell::new(i2c));
    let mut cs_pin = pins.spi2_cs;
    {
        // Hold the display CS high while the panel powers up.
        let _cs = Output::new(cs_pin.reborrow(), Level::High, OutputConfig::default());
        let mut dev = RefCellDevice::new(bus);
        chips::stopwatch_power_on(&mut dev, &mut delay).map_err(InitError::I2c)?;
    }

    let spi = Spi::new(p.SPI2, SpiConfig::default().with_frequency(Rate::from_hz(lcd::SPI_MAX_FREQUENCY)).with_mode(Mode::_0))
        .map_err(|_| InitError::Config)?
        .with_sck(pins.spi2_sck)
        .with_cs(cs_pin)
        .with_sio0(pins.spi2_sio0)
        .with_sio1(pins.spi2_sio1)
        .with_sio2(pins.spi2_sio2)
        .with_sio3(pins.spi2_sio3)
        .with_dma(p.DMA_CH0);
    let panel_bus = EspQspiDmaBus::new(spi, buf_a, buf_b);
    let cfg = dcs_config(lcd::WIDTH, lcd::HEIGHT, lcd::OFFSET, lcd::INVERT, lcd::RGB_ORDER);
    let mut panel = DcsPanel::new(panel_bus, Co5300, cfg, None::<NoPin>);
    panel.init(&mut delay).map_err(|_| InitError::Display)?;
    let mut display = kumiki_gfx::BufferedDisplay::new(
        Display::new(panel),
        embedded_graphics_core::geometry::Size::new(w as u32, h as u32),
        fb,
        Co5300::X_ALIGN,
    )
    .ok_or(InitError::Config)?;
    embedded_graphics_core::draw_target::DrawTarget::clear(&mut display, embedded_graphics_core::pixelcolor::Rgb565::new(0, 0, 0)).ok();
    display.flush().map_err(|_| InitError::Display)?;

    // Second-source touch (status = "fail-needs-probe", probe-group "touch"):
    // the first candidate that answers wins. CST9217 needs up to ~50 ms after reset.
    let mut touch = None;
    for _ in 0..8 {
        let mut dev = RefCellDevice::new(bus);
        if embedded_hal::i2c::I2c::write(&mut dev, touch_cst820::ADDR, &[0x00]).is_ok() {
            let map = chips::TouchMap {
                min: (touch_cst820::TOUCHSCREEN_MIN_X.unwrap_or(0), touch_cst820::TOUCHSCREEN_MIN_Y.unwrap_or(0)),
                size: (
                    touch_cst820::TOUCHSCREEN_SIZE_X.unwrap_or(w as u32) as i32,
                    touch_cst820::TOUCHSCREEN_SIZE_Y.unwrap_or(h as u32) as i32,
                ),
                pixels: (w as i32, h as i32),
            };
            touch = Some(AnyTouch::Cst820(chips::Cst820 { i2c: RefCellDevice::new(bus), map, inited: false }));
            break;
        }
        if embedded_hal::i2c::I2c::write(&mut dev, touch_cst9217::ADDR, &[0xD0, 0x00]).is_ok() {
            touch = Some(AnyTouch::Cst9217(chips::Cst9217 { i2c: RefCellDevice::new(bus) }));
            break;
        }
        delay.delay_millis(10);
    }
    let mut bmi = kumiki_bmi270::Bmi270::new(RefCellDevice::new(bus), imu::ADDR);
    let imu = bmi.init(&mut delay).ok().map(|_| AnyImu::Bmi270(bmi));

    let a = Input::new(pins.buttons_button_a, InputConfig::default().with_pull(Pull::Up));
    let b = Input::new(pins.buttons_button_b, InputConfig::default().with_pull(Pull::Up));

    Ok(Devices {
        board: BoardId::M5StackStopWatch,
        display: AnyDisplay::StopWatch(display),
        backlight: AnyBacklight::None,
        touch,
        imu,
        power: None,
        buttons: ButtonInputs::Gpio { a, b: Some(b) },
        spare,
    })
}
