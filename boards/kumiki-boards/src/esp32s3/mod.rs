//! ESP32-S3 board family: M5Stack CoreS3 and AtomS3.
//!
//! Detection order (no output pin of another candidate is driven before its
//! board is identified):
//! 1. CoreS3: internal I²C (SDA 12 / SCL 11) answers with AXP2101 id 0x4A at
//!    0x34 and AW9523B id 0x23 at 0x58 (as in M5GFX's autodetect).
//! 2. AtomS3: the GC9107 panel on the AtomS3 display pins answers RDDID with
//!    `00 91 07` (3-wire SPI, bit-banged), as in M5GFX's autodetect.
//!    The MPU-6886 is probed afterwards and is optional.

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
use kumiki_gfx_bus_esp::{EspSpiDmaBus, SpiHost};
use kumiki_gfx_panels::{DcsConfig, DcsPanel, Gc9107, Ili9342c, NoPin};
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

const CORES3: bool = cfg!(feature = "board-m5stack-cores3") || !cfg!(any(feature = "board-m5stack-atoms3"));
const ATOMS3: bool = cfg!(feature = "board-m5stack-atoms3") || !cfg!(any(feature = "board-m5stack-cores3"));

/// Boards of this chip family.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoardId {
    /// M5Stack CoreS3 / CoreS3 SE.
    M5StackCoreS3,
    /// M5Stack AtomS3.
    M5StackAtomS3,
}

impl BoardId {
    /// Board name.
    pub const fn name(self) -> &'static str {
        match self {
            BoardId::M5StackCoreS3 => "M5Stack CoreS3",
            BoardId::M5StackAtomS3 => "M5Stack AtomS3",
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
    /// AtomS3: the screen is a push button on GPIO41 (active low).
    Gpio(Input<'static>),
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
            ButtonInputs::Gpio(pin) => (pin.is_low() as u8) << ButtonId::A as u8,
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
    let (mut axp, mut aw, mut id) = (None, None, [0u8; 3]);
    if CORES3 {
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
    if ATOMS3 {
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
    Err(InitError::UnknownBoard { axp2101: axp, aw9523: aw, atoms3_panel_id: id })
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
    }
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
        buttons: ButtonInputs::Gpio(button),
        spare,
    })
}
