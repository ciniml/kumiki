//! ESP32-S3 board family: M5Stack CoreS3, AtomS3 and StopWatch.
//!
//! Everything board-specific comes from the device tree:
//! `boards/dts/m5stack/*.dts` + `boards/bindings/*.yaml` are compiled by
//! `kumiki-dtgen` into [`generated`] — board ids, detection, pin bundles,
//! per-device constants, the dependency-ordered bring-up (`init` per board,
//! expanded from the binding templates) and the dispatch.
//!
//! This file only defines the family's runtime types ([`Devices`],
//! [`InitError`], `Any*` in `any.rs`) and the helpers the templates call
//! (`chips`, `bind`, `probe`).
//!
//! Regenerate after editing a DTS or binding:
//! `cargo run -p kumiki-dtgen -- esp32s3 boards/kumiki-boards/src/esp32s3/generated.rs boards/dts/m5stack/cores3.dts boards/dts/m5stack/atoms3.dts boards/dts/m5stack/stopwatch.dts`

/// Move the board-independent spare peripherals out of `Peripherals`
/// (used by the generated `init` functions, hence defined before them).
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

#[macro_use]
pub mod generated;
mod any;
pub mod bind;
pub mod chips;
pub mod probe;

use embedded_hal_bus::i2c::RefCellDevice;
use esp_hal::{Blocking, dma_tx_buffer, gpio::Input, i2c::master::I2c, peripherals::Peripherals};
use kumiki_class::{ButtonId, TouchPoint};
use kumiki_gfx::Display;
use kumiki_gfx_bus_esp::{EspQspiDmaBus, EspSpiDmaBus};
use kumiki_gfx_panels::{Co5300, DcsPanel, Gc9107, Ili9342c};

pub use any::{AnyAsyncDisplay, AnyError};
pub use generated::{AnyBacklight, AnyDisplay, AnyImu, AnyPower, AnyRtc, AnyTouch, BoardId, DetectLog};

/// Push-pull output (e.g. a GPIO backlight).
pub type GpioOut = esp_hal::gpio::Output<'static>;

/// Blocking I²C master.
pub type I2cBus = I2c<'static, Blocking>;
/// A device handle on a shared (single-core) I²C bus.
pub type SharedI2c = RefCellDevice<'static, I2cBus>;
/// Panel bus used by the SPI displays of this family.
pub type PanelBus = EspSpiDmaBus<'static, Blocking>;
/// ILI9342C display (CoreS3).
pub type Ili9342cDisplay = Display<DcsPanel<PanelBus, Ili9342c>>;
/// GC9107 display (AtomS3).
pub type Gc9107Display = Display<DcsPanel<PanelBus, Gc9107>>;
/// `async` panel bus.
pub type AsyncPanelBus = EspSpiDmaBus<'static, esp_hal::Async>;
/// ILI9342C display, `async`.
pub type Ili9342cAsyncDisplay = Display<DcsPanel<AsyncPanelBus, Ili9342c>>;
/// GC9107 display, `async`.
pub type Gc9107AsyncDisplay = Display<DcsPanel<AsyncPanelBus, Gc9107>>;
/// CO5300 display (StopWatch): QSPI, frame-buffered in PSRAM.
pub type Co5300Display = kumiki_gfx::BufferedDisplay<
    Display<DcsPanel<EspQspiDmaBus<'static, Blocking>, Co5300>>,
    kumiki_gfx::Rgb565Be,
    &'static mut [u8],
>;
/// CO5300 display, `async` (push whole aligned frames from your own buffer).
pub type Co5300AsyncDisplay = Display<DcsPanel<EspQspiDmaBus<'static, esp_hal::Async>, Co5300>>;

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
    /// Active-low GPIO buttons A and (optionally) B (DT `gpio-keys`).
    Gpio {
        /// Button A.
        a: Input<'static>,
        /// Button B.
        b: Option<Input<'static>>,
    },
    /// No front buttons: a touch anywhere acts as button A (DT `kumiki,touch-keys`).
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
    /// Real-time clock, if any (DT `chosen { kumiki,rtc }`).
    pub rtc: Option<AnyRtc>,
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
    generated::init_board(board, p, buf_a, buf_b)
}
