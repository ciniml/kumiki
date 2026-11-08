//! ESP32-S3 board family: M5Stack CoreS3, AtomS3 and StopWatch.
//!
//! Detection order (no output pin of another candidate is driven before its
//! board is identified):
//! 0. Chips with embedded octal PSRAM (eFuse `PSRAM_CAP`, e.g. ESP32-S3R8)
//!    own GPIO33..37, so candidates that touch those pins (CoreS3: GPIO35/37,
//!    AtomS3: GPIO33/34) are skipped on them, as M5GFX does with
//!    `touches_opi_pins`.
//! 1. CoreS3: internal I²C (SDA 12 / SCL 11) answers with AXP2101 id 0x4A at
//!    0x34 and AW9523B id 0x23 at 0x58 (as in M5GFX's autodetect).
//! 2. AtomS3: the GC9107 panel on the AtomS3 display pins answers RDDID with
//!    `00 91 07` (3-wire SPI, bit-banged), as in M5GFX's autodetect.
//!    The MPU-6886 is probed afterwards and is optional.
//! 3. StopWatch (octal-PSRAM chips only): the M5PM1 PMIC answers at 0x6E on
//!    the internal I²C (SDA 47 / SCL 48).

mod any;
pub mod chips;
pub mod probe;

/// RDDID bytes of the GC9107 (M5GFX: `(id & 0xFFFFFF) == 0x079100`).
const GC9107_ID: [u8; 3] = [0x00, 0x91, 0x07];

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
use kumiki_gfx_panels::{Co5300, DcsConfig, DcsPanel, Gc9107, Ili9342c, NoPin};
use kumiki_mpu6886::Mpu6886;
use static_cell::StaticCell;

pub use any::{AnyAsyncDisplay, AnyBacklight, AnyDisplay, AnyImu, AnyPower, AnyTouch};

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

const ANY_PINNED: bool = cfg!(any(
    feature = "board-m5stack-cores3",
    feature = "board-m5stack-atoms3",
    feature = "board-m5stack-stopwatch"
));
const CORES3: bool = !ANY_PINNED || cfg!(feature = "board-m5stack-cores3");
const ATOMS3: bool = !ANY_PINNED || cfg!(feature = "board-m5stack-atoms3");
const STOPWATCH: bool = !ANY_PINNED || cfg!(feature = "board-m5stack-stopwatch");

/// Boards of this chip family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoardId {
    /// M5Stack CoreS3 / CoreS3 SE.
    M5StackCoreS3,
    /// M5Stack AtomS3.
    M5StackAtomS3,
    /// M5Stack StopWatch.
    M5StackStopWatch,
}

impl BoardId {
    /// Board name.
    pub const fn name(self) -> &'static str {
        match self {
            BoardId::M5StackCoreS3 => "M5Stack CoreS3",
            BoardId::M5StackAtomS3 => "M5Stack AtomS3",
            BoardId::M5StackStopWatch => "M5Stack StopWatch",
        }
    }
}

/// Bring-up error.
#[derive(Debug)]
pub enum InitError {
    /// No candidate board matched. Holds what each probe read (`None` = no ACK).
    UnknownBoard {
        /// AXP2101 register 0x03 on I²C 12/11.
        axp2101: Option<u8>,
        /// AW9523B register 0x10 on I²C 12/11.
        aw9523: Option<u8>,
        /// RDDID bytes read on the AtomS3 display pins.
        atoms3_panel_id: [u8; 3],
        /// Whether the chip has embedded octal PSRAM (eFuse PSRAM_CAP).
        octal_psram: bool,
        /// M5PM1 register 0x00 on I²C 47/48.
        pm1: Option<u8>,
    },
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
    /// AtomS3: the screen button on GPIO41. StopWatch: A = GPIO2, B = GPIO1.
    Gpio {
        /// Button A.
        a: Input<'static>,
        /// Button B.
        b: Option<Input<'static>>,
    },
    /// CoreS3: no front buttons; a touch anywhere acts as button A.
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
    /// IMU, if supported yet (CoreS3's BMI270 is pending).
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

/// Probe which board this is, borrowing the peripherals only for the duration.
fn detect(p: &mut Peripherals) -> Result<BoardId, InitError> {
    let cfg = I2cConfig::default().with_frequency(Rate::from_khz(100));
    let (mut axp, mut aw, mut id, mut pm1) = (None, None, [0u8; 3], None);
    // PSRAM_CAP: 0 = none, 2 = 2 MB quad; other values are embedded octal PSRAM.
    let cap: u8 = esp_hal::efuse::read_field_le(esp_hal::efuse::PSRAM_CAP);
    let octal_psram = cap != 0 && cap != 2;
    if STOPWATCH && octal_psram {
        probe::i2c_bus_recover(p.GPIO48.reborrow(), p.GPIO47.reborrow());
        let mut i2c = I2c::new(p.I2C1.reborrow(), cfg)
            .map_err(|_| InitError::Config)?
            .with_sda(p.GPIO47.reborrow())
            .with_scl(p.GPIO48.reborrow());
        pm1 = chips::read_reg(&mut i2c, chips::PM1_ADDR, 0x00).ok();
        if pm1.is_some() {
            return Ok(BoardId::M5StackStopWatch);
        }
    }
    if CORES3 && !octal_psram {
        probe::i2c_bus_recover(p.GPIO11.reborrow(), p.GPIO12.reborrow());
        let mut i2c = I2c::new(p.I2C0.reborrow(), cfg)
            .map_err(|_| InitError::Config)?
            .with_sda(p.GPIO12.reborrow())
            .with_scl(p.GPIO11.reborrow());
        axp = chips::read_reg(&mut i2c, chips::AXP2101_ADDR, 0x03).ok();
        aw = chips::read_reg(&mut i2c, chips::AW9523_ADDR, 0x10).ok();
        if axp == Some(chips::AXP2101_ID) && aw == Some(chips::AW9523_ID) {
            return Ok(BoardId::M5StackCoreS3);
        }
    }
    if ATOMS3 && !octal_psram {
        // GC9107 panel id over the AtomS3 display pins (SCK 17, SDA 21, CS 15, DC 33, RST 34).
        id = probe::read_dcs_id_3wire(
            p.GPIO17.reborrow(),
            p.GPIO21.reborrow(),
            p.GPIO15.reborrow(),
            p.GPIO33.reborrow(),
            p.GPIO34.reborrow(),
        );
        if id == GC9107_ID {
            return Ok(BoardId::M5StackAtomS3);
        }
    }
    Err(InitError::UnknownBoard { axp2101: axp, aw9523: aw, atoms3_panel_id: id, octal_psram, pm1 })
}

/// Detect the board and bring every device up. Call once, after `esp_hal::init`.
///
/// Allocate large buffers (frame buffers, sprites) before starting Wi-Fi.
pub fn init(mut p: Peripherals) -> Result<Devices, InitError> {
    let board = detect(&mut p)?;
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
fn init_stopwatch(mut p: Peripherals, buf_a: DmaTxBuf, buf_b: DmaTxBuf) -> Result<Devices, InitError> {
    let spare = take_spare!(p);
    let mut delay = Delay::new();

    // Frame buffer in octal PSRAM (468×468×2 ≈ 428 KiB).
    let psram = esp_hal::psram::Psram::new(p.PSRAM, esp_hal::psram::PsramConfig::default());
    let (ptr, len) = psram.raw_parts();
    core::mem::forget(psram); // keep the mapping for the program's lifetime
    const W: u16 = 468;
    const H: u16 = 468;
    let fb_len = W as usize * H as usize * 2;
    if len < fb_len {
        return Err(InitError::Config);
    }
    // SAFETY: PSRAM is mapped for the program's lifetime and used only here.
    let fb: &'static mut [u8] = unsafe { core::slice::from_raw_parts_mut(ptr, fb_len) };

    // i2c_int → pm1 → ioe1 (LCD/touch power + reset) → panel → touch, imu
    let i2c = I2c::new(p.I2C1, I2cConfig::default().with_frequency(Rate::from_khz(100)))
        .map_err(|_| InitError::Config)?
        .with_sda(p.GPIO47)
        .with_scl(p.GPIO48);
    let bus: &'static RefCell<I2cBus> = I2C_INT.init(RefCell::new(i2c));
    {
        // Hold the display CS high while the panel powers up.
        let _cs = Output::new(p.GPIO39.reborrow(), Level::High, OutputConfig::default());
        let mut dev = RefCellDevice::new(bus);
        chips::stopwatch_power_on(&mut dev, &mut delay).map_err(InitError::I2c)?;
    }

    let spi = Spi::new(p.SPI2, SpiConfig::default().with_frequency(Rate::from_mhz(40)).with_mode(Mode::_0))
        .map_err(|_| InitError::Config)?
        .with_sck(p.GPIO40)
        .with_cs(p.GPIO39)
        .with_sio0(p.GPIO41)
        .with_sio1(p.GPIO42)
        .with_sio2(p.GPIO46)
        .with_sio3(p.GPIO45)
        .with_dma(p.DMA_CH0);
    let panel_bus = EspQspiDmaBus::new(spi, buf_a, buf_b);
    // 466×466 round panel; M5GFX uses 468×468 at x offset 6 in a 480×480 memory.
    let cfg = DcsConfig { panel_size: (W, H), offset: (6, 0), offset_rotation: 0, invert: false, rgb_order: true };
    let mut panel = DcsPanel::new(panel_bus, Co5300, cfg, None::<NoPin>);
    panel.init(&mut delay).map_err(|_| InitError::Display)?;
    let mut display = kumiki_gfx::BufferedDisplay::new(Display::new(panel), embedded_graphics_core::geometry::Size::new(W as u32, H as u32), fb, 2)
        .ok_or(InitError::Config)?;
    embedded_graphics_core::draw_target::DrawTarget::clear(&mut display, embedded_graphics_core::pixelcolor::Rgb565::new(0, 0, 0)).ok();
    display.flush().map_err(|_| InitError::Display)?;

    // Units ship with CST820 (0x15) or CST9217 (0x5A); the latter needs up to
    // ~50 ms after reset before it answers (M5GFX `stopwatch_touch_is_cst9217`).
    let mut touch = None;
    for _ in 0..8 {
        let mut dev = RefCellDevice::new(bus);
        if embedded_hal::i2c::I2c::write(&mut dev, chips::CST820_ADDR, &[0x00]).is_ok() {
            touch = Some(AnyTouch::Cst820(chips::Cst820 { i2c: RefCellDevice::new(bus), scale: 2, inited: false }));
            break;
        }
        if embedded_hal::i2c::I2c::write(&mut dev, chips::CST9217_ADDR, &[0xD0, 0x00]).is_ok() {
            touch = Some(AnyTouch::Cst9217(chips::Cst9217 { i2c: RefCellDevice::new(bus) }));
            break;
        }
        delay.delay_millis(10);
    }
    let mut bmi = kumiki_bmi270::Bmi270::new(RefCellDevice::new(bus), kumiki_bmi270::DEFAULT_ADDRESS);
    let imu = bmi.init(&mut delay).ok().map(|_| AnyImu::Bmi270(bmi));

    let a = Input::new(p.GPIO2, InputConfig::default().with_pull(Pull::Up));
    let b = Input::new(p.GPIO1, InputConfig::default().with_pull(Pull::Up));

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

#[inline(never)]
fn init_cores3(p: Peripherals, buf_a: DmaTxBuf, buf_b: DmaTxBuf) -> Result<Devices, InitError> {
    let spare = take_spare!(p);
    let mut delay = Delay::new();
    // i2c_int → (pmic, ioexp) → lcd(reset, supply) → backlight → touch
    let i2c = I2c::new(p.I2C0, I2cConfig::default().with_frequency(Rate::from_khz(400)))
        .map_err(|_| InitError::Config)?
        .with_sda(p.GPIO12)
        .with_scl(p.GPIO11);
    let bus: &'static RefCell<I2cBus> = I2C_INT.init(RefCell::new(i2c));
    {
        let mut dev = RefCellDevice::new(bus);
        chips::aw9523_init_cores3(&mut dev, false).map_err(InitError::I2c)?;
        chips::axp2101_power_on_cores3(&mut dev).map_err(InitError::I2c)?;
        delay.delay_millis(10);
        chips::aw9523_init_cores3(&mut dev, true).map_err(InitError::I2c)?; // release LCD_RST
        delay.delay_millis(120);
    }

    let spi = Spi::new(p.SPI2, SpiConfig::default().with_frequency(Rate::from_mhz(40)).with_mode(Mode::_0))
        .map_err(|_| InitError::Config)?
        .with_sck(p.GPIO36)
        .with_mosi(p.GPIO37)
        .with_dma(p.DMA_CH0);
    let dc = Output::new(p.GPIO35, Level::High, OutputConfig::default()); // shared with MISO (unused)
    let cs = Output::new(p.GPIO3, Level::High, OutputConfig::default());
    let panel_bus = EspSpiDmaBus::new(spi, dc, Some(cs), buf_a, buf_b).with_fifo(SpiHost::Spi2);
    let cfg = DcsConfig { invert: true, ..DcsConfig::for_model::<Ili9342c>() };
    let mut panel = DcsPanel::new(panel_bus, Ili9342c, cfg, None::<NoPin>);
    panel.init(&mut delay).map_err(|_| InitError::Display)?;
    let mut display = Display::new(panel);
    embedded_graphics_core::draw_target::DrawTarget::clear(&mut display, embedded_graphics_core::pixelcolor::Rgb565::new(0, 0, 0))
        .map_err(|_| InitError::Display)?;

    let mut backlight = AnyBacklight::Axp2101(chips::Axp2101Backlight(RefCellDevice::new(bus)));
    kumiki_class::Backlight::set_brightness(&mut backlight, 160).map_err(|_| InitError::Display)?;

    let mut bmi = kumiki_bmi270::Bmi270::new(RefCellDevice::new(bus), kumiki_bmi270::ALT_ADDRESS);
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
fn init_atoms3(mut p: Peripherals, buf_a: DmaTxBuf, buf_b: DmaTxBuf) -> Result<Devices, InitError> {
    let spare = take_spare!(p);
    let mut delay = Delay::new();
    let mut rst = Output::new(p.GPIO34, Level::Low, OutputConfig::default());
    delay.delay_millis(10);
    rst.set_high();
    delay.delay_millis(120);

    let spi = Spi::new(p.SPI2, SpiConfig::default().with_frequency(Rate::from_mhz(40)).with_mode(Mode::_0))
        .map_err(|_| InitError::Config)?
        .with_sck(p.GPIO17)
        .with_mosi(p.GPIO21)
        .with_dma(p.DMA_CH0);
    let dc = Output::new(p.GPIO33, Level::High, OutputConfig::default());
    let cs = Output::new(p.GPIO15, Level::High, OutputConfig::default());
    let panel_bus = EspSpiDmaBus::new(spi, dc, Some(cs), buf_a, buf_b).with_fifo(SpiHost::Spi2);
    let cfg = DcsConfig { panel_size: (128, 128), offset: (0, 32), ..DcsConfig::for_model::<Gc9107>() };
    let mut panel = DcsPanel::new(panel_bus, Gc9107, cfg, None::<NoPin>);
    panel.init(&mut delay).map_err(|_| InitError::Display)?;
    let mut display = Display::new(panel);
    embedded_graphics_core::draw_target::DrawTarget::clear(&mut display, embedded_graphics_core::pixelcolor::Rgb565::new(0, 0, 0))
        .map_err(|_| InitError::Display)?;
    // Keep the reset line released for the lifetime of the program.
    core::mem::forget(rst);

    let backlight = AnyBacklight::Gpio(Output::new(p.GPIO16, Level::High, OutputConfig::default()));

    // The IMU stays powered across MCU resets and could hold SDA low after an
    // interrupted transfer; recover the bus before use.
    probe::i2c_bus_recover(p.GPIO39.reborrow(), p.GPIO38.reborrow());
    let i2c = I2c::new(p.I2C1, I2cConfig::default().with_frequency(Rate::from_khz(400)))
        .map_err(|_| InitError::Config)?
        .with_sda(p.GPIO38)
        .with_scl(p.GPIO39);
    let mut imu = Mpu6886::new(i2c, kumiki_mpu6886::DEFAULT_ADDRESS);
    let imu = imu.init(&mut delay).ok().map(|_| AnyImu::Mpu6886(imu));

    let button = Input::new(p.GPIO41, InputConfig::default().with_pull(Pull::Up));

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
