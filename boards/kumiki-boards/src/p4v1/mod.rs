//! ESP32-P4 rev v1 board family (kumiki-p4v1-hal): M5Stack Tab5.
//!
//! As for the ESP32-S3 family, everything board-specific comes from the
//! device tree: `boards/dts/m5stack/tab5.dts` + `boards/bindings/*.yaml` are
//! compiled by `kumiki-dtgen p4v1` into [`generated`]. This file defines the
//! family's runtime types and the chip bring-up that precedes any board code
//! (clock, cache, PSRAM).
//!
//! Regenerate after editing a DTS or binding:
//! `cargo run -p kumiki-dtgen -- p4v1 boards/kumiki-boards/src/p4v1/generated.rs boards/dts/m5stack/tab5.dts`

/// The family has no board-independent spare peripherals yet.
macro_rules! take_spare {
    ($p:ident) => {
        Spare {}
    };
}

#[macro_use]
pub mod generated;
mod any;
pub mod audio;
pub mod bind;
pub mod chips;
pub mod probe;

use embedded_hal_bus::i2c::RefCellDevice;
use kumiki_class::{ButtonId, TouchPoint};
use kumiki_p4v1_hal::{Peripherals, cache, clock, dsi, i2c::I2c, psram};

pub use any::{AnyError, DisplayError};
pub use generated::{
    AnyBacklight, AnyDisplay, AnyImu, AnyMicrophone, AnyPower, AnyRtc, AnySpeaker, AnyTouch, BoardId, DetectLog,
};

/// Push-pull output (e.g. a GPIO backlight).
pub type GpioOut = kumiki_p4v1_hal::gpio::Output;

/// Blocking I²C master.
pub type I2cBus = I2c;
/// A device handle on a shared (single-core) I²C bus.
pub type SharedI2c = RefCellDevice<'static, I2cBus>;

/// Bring-up error.
#[derive(Debug)]
pub enum InitError {
    /// No candidate board matched; what each probe read.
    UnknownBoard(DetectLog),
    /// PSRAM bring-up failed.
    Psram(&'static str),
    /// A peripheral could not be configured.
    Config,
    /// A required device did not answer.
    NoDevice(&'static str),
    /// The panel lot is not supported yet (Tab5 ST7121 / ST7123).
    UnsupportedPanel,
    /// Panel initialization failed.
    Display,
}

/// Raw button sources of the detected board.
pub enum ButtonInputs {
    /// No front buttons: a touch anywhere acts as button A (DT `kumiki,touch-keys`).
    TouchAsA,
}

impl ButtonInputs {
    /// Raw state bits for [`kumiki_class::Buttons::update`].
    pub fn raw(&mut self, touch: Option<&mut AnyTouch>) -> u8 {
        match self {
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

/// Peripherals not used by any board of this family (none yet).
pub struct Spare {}

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
    /// Peripherals the board definition does not use.
    pub spare: Spare,
}

/// Reset reason of this boot (read by the generated `init` before bring-up;
/// doc/13 §4.4). The wake-up cause of a deep-sleep wake is not decoded yet.
pub fn boot_info() -> kumiki_class::BootInfo {
    use kumiki_class::{BootInfo, ResetReason, WakeCause};
    let reset = crate::espressif::reset_reason(kumiki_p4v1_hal::reset::reason_code());
    let wake = if reset == ResetReason::DeepSleep { WakeCause::Unknown } else { WakeCause::NotFromSleep };
    BootInfo { reset, wake }
}

/// Bring the chip up (cache line size, PSRAM, 360 MHz CPU), detect the
/// board and bring every device up. Call once, first thing in `main`.
pub fn init(mut p: Peripherals) -> Result<Devices, InitError> {
    // A previous program may have left the frame DMA reading PSRAM.
    dsi::stop();
    cache::set_l2_line();
    let psram = psram::init().map_err(InitError::Psram)?;
    // Without PVT calibration the clock stays at the bootloader's 90 MHz.
    let _ = clock::init_max();
    let board = generated::detect(&mut p, false, true).map_err(InitError::UnknownBoard)?;
    let mut dev = generated::init_board(board, p, psram)?;
    // The Tab5's ES8388 stays silent after the init the bring-up does
    // (before the display power sequence, which it must precede) unless its
    // power is cycled afterwards (found on hardware by kumiki and tab5-os;
    // cause unknown, see `kumiki_es8388::Es8388::power_down`). `restart` does
    // that (and nothing on speakers that need no such step).
    if let Some(s) = dev.speaker.as_mut() {
        use kumiki_class::Speaker as _;
        let _ = s.restart();
    }
    Ok(dev)
}
