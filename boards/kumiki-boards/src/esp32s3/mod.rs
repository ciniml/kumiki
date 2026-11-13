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
//! `cargo run -p kumiki-dtgen -- esp32s3 boards/kumiki-boards/src/esp32s3/generated.rs boards/dts/m5stack/cores3-stackchan.dts boards/dts/m5stack/cores3.dts boards/dts/m5stack/atoms3.dts boards/dts/m5stack/stopwatch.dts`
//! (a board with an add-on base comes before the bare board: detection takes the first match).

#[macro_use]
pub mod generated;
mod any;
pub mod audio;
pub mod bind;
pub mod chips;
pub mod probe;

use embedded_hal_bus::i2c::RefCellDevice;
use esp_hal::{
    Blocking,
    dma::{DmaRxStreamBuf, DmaTxBuf, DmaTxStreamBuf},
    gpio::Input,
    i2c::master::I2c,
    peripherals::Peripherals,
};
use kumiki_class::{ButtonId, TouchPoint};
use kumiki_gfx::Display;
use kumiki_gfx_bus_esp::{EspQspiDmaBus, EspSpiDmaBus};
use kumiki_gfx_panels::{Co5300, DcsPanel, Gc9107, Ili9342c};

pub use any::{AnyAsyncDisplay, AnyError};
pub use generated::{
    AnyBacklight, AnyDisplay, AnyImu, AnyMicrophone, AnyPower, AnyRtc, AnySpeaker, AnyTouch, BoardId, DetectLog, Extras, Free,
    MAX_BUFFERS, Spare, buffer_needs,
};

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
    /// The board is not a detection candidate of this build (`board-*` features).
    NotCandidate(BoardId),
    /// A device needs a buffer the caller did not supply in [`Buffers`].
    MissingBuffer(&'static str),
    /// A peripheral could not be configured.
    Config,
    /// I²C failure during bring-up.
    I2c(esp_hal::i2c::master::Error),
    /// Panel initialization failed.
    Display,
}

/// Buffer sizes a board's devices take from [`Buffers`] (generated from the
/// bindings' `kumiki.buffers`; doc/15 §1). Zero: not used.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct BufferNeeds {
    /// Bytes of each of the panel bus's two DMA buffers.
    pub display: usize,
    /// I²S transmit ring: (bytes, bytes per descriptor).
    pub i2s_tx: (usize, usize),
    /// I²S receive ring: (bytes, bytes per descriptor).
    pub i2s_rx: (usize, usize),
}

impl BufferNeeds {
    /// No buffers.
    pub const NONE: Self = Self { display: 0, i2s_tx: (0, 0), i2s_rx: (0, 0) };

    /// Element-wise larger of the two.
    pub const fn max(self, o: Self) -> Self {
        const fn m(a: usize, b: usize) -> usize {
            if a > b { a } else { b }
        }
        Self {
            display: m(self.display, o.display),
            i2s_tx: (m(self.i2s_tx.0, o.i2s_tx.0), m(self.i2s_tx.1, o.i2s_tx.1)),
            i2s_rx: (m(self.i2s_rx.0, o.i2s_rx.0), m(self.i2s_rx.1, o.i2s_rx.1)),
        }
    }
}

/// DMA buffers handed to [`init_with`]: in internal RAM, `'static`. Give the
/// ones [`buffer_needs`] lists for the board (others are not used). Sizes
/// may differ from the defaults: smaller display buffers work, more slowly;
/// a ring needs at least 4 descriptors.
#[derive(Default)]
pub struct Buffers {
    /// The panel bus's two DMA buffers.
    pub display: Option<(DmaTxBuf, DmaTxBuf)>,
    /// I²S transmit ring (speaker).
    pub i2s_tx: Option<DmaTxStreamBuf>,
    /// I²S receive ring (microphone).
    pub i2s_rx: Option<DmaRxStreamBuf>,
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
    /// How the chip started (cold boot, reset, deep-sleep wake).
    pub boot: kumiki_class::BootInfo,
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
    /// Audio output, if any (DT `chosen { kumiki,speaker }`).
    pub speaker: Option<AnySpeaker>,
    /// Audio input, if any (DT `chosen { kumiki,microphone }`).
    pub microphone: Option<AnyMicrophone>,
    /// Button raw-state source.
    pub buttons: ButtonInputs,
    /// The I²C bus the board devices share (DT `chosen { kumiki,i2c }`, or
    /// the board's only bus). Add devices with `RefCellDevice::new(i2c)`;
    /// single-core: use it from the core that runs the board devices.
    pub i2c: Option<&'static core::cell::RefCell<I2cBus>>,
    /// Chip GPIOs and peripheral instances this board leaves unused (only
    /// some boards of the family use them). Pins wired to hardware the
    /// device tree does not describe are listed too.
    pub free: Free,
    /// Peripherals no board of the family uses (generated), handed back to
    /// the application (timers and software interrupts for an RTOS/executor,
    /// radio, RNG, second core, ...).
    pub spare: Spare,
    /// Board-specific devices (DT `chosen { kumiki,extras }`), e.g. an
    /// add-on base's expander, supplies and buses.
    pub extras: Extras,
    /// The board's PSRAM (DT `psram` node, mapped in its `kumiki,mode`) less
    /// what the devices carved out (a frame buffer). `None` if the board
    /// declares none or it did not come up; then `free.psram` may hold the
    /// peripheral. Hand it to a heap (`kumiki-boards-alloc`) or use it as is.
    pub psram: Option<&'static mut [u8]>,
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

/// Reset reason and sleep wake-up cause of this boot (read by the generated
/// `init` before bring-up; doc/13 §4.4).
pub fn boot_info() -> kumiki_class::BootInfo {
    use esp_hal::rtc_cntl::{WakeupSource as W, reset_reason, wakeup_cause};
    use kumiki_class::{BootInfo, WakeCause};
    let code = reset_reason(esp_hal::system::Cpu::ProCpu).map(|r| r as u8).unwrap_or(0);
    let w = wakeup_cause();
    let wake = if w.is_empty() {
        WakeCause::NotFromSleep
    } else if w.contains(W::Timer) {
        WakeCause::Timer
    } else if w.contains(W::Ext0) || w.contains(W::Ext1) || w.contains(W::Gpio) {
        WakeCause::Gpio
    } else if w.contains(W::Uart0) || w.contains(W::Uart1) {
        WakeCause::Uart
    } else if w.contains(W::Touch) {
        WakeCause::Touch
    } else {
        WakeCause::Unknown
    };
    BootInfo { reset: crate::espressif::reset_reason(code), wake }
}

/// (any PSRAM, embedded octal PSRAM) from eFuse PSRAM_CAP: 0 = none,
/// 2 = 2 MB quad; other values are embedded octal PSRAM.
fn psram_cap() -> (bool, bool) {
    let cap: u8 = esp_hal::efuse::read_field_le(esp_hal::efuse::PSRAM_CAP);
    (cap != 0, cap != 0 && cap != 2)
}

/// Detect the board. `Peripherals` are only borrowed, so an unknown board
/// leaves them all to the application.
pub fn detect(p: &mut Peripherals) -> Result<BoardId, InitError> {
    let (psram, octal_psram) = psram_cap();
    generated::detect(p, octal_psram, psram).map_err(InitError::UnknownBoard)
}

/// Bring up `board` (from [`detect`]) with the caller's DMA buffers
/// ([`buffer_needs`] lists what it takes). Call once, after `esp_hal::init`.
pub fn init_with(p: Peripherals, board: BoardId, bufs: Buffers) -> Result<Devices, InitError> {
    let mut dev = generated::init_board(board, p, bufs)?;
    if psram_cap().1 {
        dev.free.withhold_octal_psram_pins();
    }
    Ok(dev)
}

/// Detect the board and bring every device up, with DMA buffers kumiki
/// keeps in `static`s: one set, as large as the largest detection candidate
/// needs ([`MAX_BUFFERS`]). Call once, after `esp_hal::init`.
///
/// To size or place the buffers yourself, use [`detect`] + [`init_with`]
/// (or `kumiki-boards-alloc`). Allocate large buffers (frame buffers,
/// sprites) before starting Wi-Fi.
pub fn init(mut p: Peripherals) -> Result<Devices, InitError> {
    const fn chunk(n: usize) -> usize {
        if n == 0 { 1 } else { n }
    }
    let board = detect(&mut p)?;
    let need = buffer_needs(board);
    let mut bufs = Buffers::default();
    if need.display > 0 {
        let a = esp_hal::dma_tx_buffer!(MAX_BUFFERS.display).map_err(|_| InitError::Config)?;
        let b = esp_hal::dma_tx_buffer!(MAX_BUFFERS.display).map_err(|_| InitError::Config)?;
        bufs.display = Some((a, b));
    }
    if need.i2s_tx.0 > 0 {
        bufs.i2s_tx = Some(esp_hal::dma_tx_stream_buffer!(MAX_BUFFERS.i2s_tx.0, chunk(MAX_BUFFERS.i2s_tx.1)));
    }
    if need.i2s_rx.0 > 0 {
        bufs.i2s_rx = Some(esp_hal::dma_rx_stream_buffer!(MAX_BUFFERS.i2s_rx.0, chunk(MAX_BUFFERS.i2s_rx.1)));
    }
    init_with(p, board, bufs)
}
