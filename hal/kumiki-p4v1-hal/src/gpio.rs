//! GPIO pins through the IO MUX and the GPIO matrix (gpio_reg.h,
//! io_mux_reg.h, gpio_sig_map.h of `hw_ver1`).

use core::convert::Infallible;

use crate::{clear_bits, rd, set_bits, wr};

const GPIO_BASE: usize = 0x500E_0000;
const OUT_W1TS: usize = GPIO_BASE + 0x08;
const OUT_W1TC: usize = GPIO_BASE + 0x0C;
const OUT1_W1TS: usize = GPIO_BASE + 0x14;
const OUT1_W1TC: usize = GPIO_BASE + 0x18;
const OUT: usize = GPIO_BASE + 0x04;
const OUT1: usize = GPIO_BASE + 0x10;
const ENABLE_W1TS: usize = GPIO_BASE + 0x24;
const ENABLE_W1TC: usize = GPIO_BASE + 0x28;
const ENABLE1_W1TS: usize = GPIO_BASE + 0x30;
const ENABLE1_W1TC: usize = GPIO_BASE + 0x34;
const IN: usize = GPIO_BASE + 0x3C;
const IN1: usize = GPIO_BASE + 0x40;
/// GPIO_PINn_REG.PAD_DRIVER: open drain.
const PIN_PAD_DRIVER: u32 = 1 << 2;
/// FUNCn_OUT_SEL = SIG_GPIO_OUT_IDX: the pin is driven by GPIO_OUT.
const SIG_GPIO_OUT: u32 = 256;
/// FUNCn_OUT_SEL_CFG.OE_SEL: output enable from GPIO_ENABLE.
const OUT_SEL_OE_FROM_ENABLE: u32 = 1 << 10;
/// FUNCn_IN_SEL_CFG.SIG_IN_SEL: through the matrix.
const SIG_IN_SEL: u32 = 1 << 7;

const IO_MUX_FUN_PD: u32 = 1 << 7;
const IO_MUX_FUN_PU: u32 = 1 << 8;
const IO_MUX_FUN_IE: u32 = 1 << 9;
const IO_MUX_DRV_SHIFT: u32 = 10;
/// MCU_SEL = 1: the GPIO function (through the matrix).
const IO_MUX_MCU_SEL_GPIO: u32 = 1 << 12;

const fn pin_reg(n: u8) -> usize {
    GPIO_BASE + 0x74 + 4 * n as usize
}
const fn io_mux_reg(n: u8) -> usize {
    0x500E_1004 + 4 * n as usize
}
const fn func_out_sel(n: u8) -> usize {
    GPIO_BASE + 0x558 + 4 * n as usize
}
const fn func_in_sel(sig: u16) -> usize {
    GPIO_BASE + 0x158 + 4 * sig as usize
}

/// Number of GPIOs (0..=54).
pub const NUM_PINS: u8 = 55;

/// A GPIO pin, owned.
#[derive(Debug)]
pub struct AnyPin(u8);

impl AnyPin {
    /// The GPIO number.
    pub fn number(&self) -> u8 {
        self.0
    }

    /// A pin by number; `None` if there is no such GPIO.
    ///
    /// # Safety
    /// The caller must make sure nothing else uses the pin.
    pub unsafe fn steal(n: u8) -> Option<Self> {
        (n < NUM_PINS).then_some(AnyPin(n))
    }

    fn mask(&self) -> (usize, usize, usize, u32) {
        if self.0 < 32 {
            (OUT_W1TS, OUT_W1TC, IN, 1 << self.0)
        } else {
            (OUT1_W1TS, OUT1_W1TC, IN1, 1 << (self.0 - 32))
        }
    }

    pub(crate) fn set_level(&self, high: bool) {
        let (s, c, _, m) = self.mask();
        unsafe { wr(if high { s } else { c }, m) };
    }

    pub(crate) fn is_set_high(&self) -> bool {
        let (r, m) = if self.0 < 32 { (OUT, 1 << self.0) } else { (OUT1, 1 << (self.0 - 32)) };
        unsafe { rd(r) & m != 0 }
    }

    pub(crate) fn input_level(&self) -> bool {
        let (_, _, i, m) = self.mask();
        unsafe { rd(i) & m != 0 }
    }

    pub(crate) fn set_output_enable(&self, on: bool) {
        let (reg, m) = match (self.0 < 32, on) {
            (true, true) => (ENABLE_W1TS, 1 << self.0),
            (true, false) => (ENABLE_W1TC, 1 << self.0),
            (false, true) => (ENABLE1_W1TS, 1 << (self.0 - 32)),
            (false, false) => (ENABLE1_W1TC, 1 << (self.0 - 32)),
        };
        unsafe { wr(reg, m) };
    }

    /// GPIO function, input buffer on, the given pulls, drive strength 2.
    pub(crate) fn configure(&self, pull: Pull, open_drain: bool) {
        let pulls = match pull {
            Pull::None => 0,
            Pull::Up => IO_MUX_FUN_PU,
            Pull::Down => IO_MUX_FUN_PD,
        };
        unsafe { wr(io_mux_reg(self.0), IO_MUX_MCU_SEL_GPIO | IO_MUX_FUN_IE | (2 << IO_MUX_DRV_SHIFT) | pulls) };
        if open_drain {
            set_bits(pin_reg(self.0), PIN_PAD_DRIVER);
        } else {
            clear_bits(pin_reg(self.0), PIN_PAD_DRIVER);
        }
    }

    /// Routes peripheral output signal `sig` to this pin (output enable
    /// from the peripheral).
    pub(crate) fn connect_output(&self, sig: u16) {
        unsafe { wr(func_out_sel(self.0), sig as u32) };
    }

    /// Routes peripheral output signal `sig` to this pin, with the output
    /// enable taken from GPIO_ENABLE (set it with `set_output_enable`)
    /// instead of from the peripheral.
    pub(crate) fn connect_output_gpio_oe(&self, sig: u16) {
        unsafe { wr(func_out_sel(self.0), sig as u32 | OUT_SEL_OE_FROM_ENABLE) };
    }

    /// Routes this pin to peripheral input signal `sig`.
    pub(crate) fn connect_input(&self, sig: u16) {
        unsafe { wr(func_in_sel(sig), self.0 as u32 | SIG_IN_SEL) };
    }

    fn connect_gpio_out(&self) {
        unsafe { wr(func_out_sel(self.0), SIG_GPIO_OUT | OUT_SEL_OE_FROM_ENABLE) };
    }
}

/// Pin `N`, owned.
#[derive(Debug)]
pub struct Pin<const N: u8>(());

impl<const N: u8> Pin<N> {
    /// Forgets the pin number in the type.
    pub fn degrade(self) -> AnyPin {
        AnyPin(N)
    }
}

impl<const N: u8> From<Pin<N>> for AnyPin {
    fn from(_: Pin<N>) -> Self {
        AnyPin(N)
    }
}

macro_rules! pins {
    ($($name:ident = $n:literal),* $(,)?) => {
        /// All GPIO pins.
        #[allow(missing_docs)]
        pub struct Pins { $(pub $name: Pin<$n>,)* }
        impl Pins {
            pub(crate) fn new() -> Self { Pins { $($name: Pin(()),)* } }
        }
    };
}

pins!(
    gpio0 = 0, gpio1 = 1, gpio2 = 2, gpio3 = 3, gpio4 = 4, gpio5 = 5, gpio6 = 6, gpio7 = 7,
    gpio8 = 8, gpio9 = 9, gpio10 = 10, gpio11 = 11, gpio12 = 12, gpio13 = 13, gpio14 = 14, gpio15 = 15,
    gpio16 = 16, gpio17 = 17, gpio18 = 18, gpio19 = 19, gpio20 = 20, gpio21 = 21, gpio22 = 22, gpio23 = 23,
    gpio24 = 24, gpio25 = 25, gpio26 = 26, gpio27 = 27, gpio28 = 28, gpio29 = 29, gpio30 = 30, gpio31 = 31,
    gpio32 = 32, gpio33 = 33, gpio34 = 34, gpio35 = 35, gpio36 = 36, gpio37 = 37, gpio38 = 38, gpio39 = 39,
    gpio40 = 40, gpio41 = 41, gpio42 = 42, gpio43 = 43, gpio44 = 44, gpio45 = 45, gpio46 = 46, gpio47 = 47,
    gpio48 = 48, gpio49 = 49, gpio50 = 50, gpio51 = 51, gpio52 = 52, gpio53 = 53, gpio54 = 54,
);

/// Internal pull resistor.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pull {
    /// No pull.
    None,
    /// Pull-up.
    Up,
    /// Pull-down.
    Down,
}

/// Push-pull output.
pub struct Output(AnyPin);

impl Output {
    /// Configures `pin` as a push-pull output at `high`.
    pub fn new(pin: impl Into<AnyPin>, high: bool) -> Self {
        let pin = pin.into();
        pin.set_level(high);
        pin.configure(Pull::None, false);
        pin.connect_gpio_out();
        pin.set_output_enable(true);
        Output(pin)
    }

    /// Turns the pin into an input.
    pub fn into_input(self, pull: Pull) -> Input {
        Input::new(self.0, pull)
    }
}

/// Input.
pub struct Input(AnyPin);

impl Input {
    /// Configures `pin` as an input.
    pub fn new(pin: impl Into<AnyPin>, pull: Pull) -> Self {
        let pin = pin.into();
        pin.set_output_enable(false);
        pin.configure(pull, false);
        Input(pin)
    }

    /// Whether the pin reads high.
    pub fn is_high(&self) -> bool {
        self.0.input_level()
    }

    /// Whether the pin reads low.
    pub fn is_low(&self) -> bool {
        !self.0.input_level()
    }

    /// Turns the pin into a push-pull output at `high`.
    pub fn into_output(self, high: bool) -> Output {
        Output::new(self.0, high)
    }
}

impl embedded_hal::digital::ErrorType for Output {
    type Error = Infallible;
}
impl embedded_hal::digital::OutputPin for Output {
    fn set_low(&mut self) -> Result<(), Infallible> {
        self.0.set_level(false);
        Ok(())
    }
    fn set_high(&mut self) -> Result<(), Infallible> {
        self.0.set_level(true);
        Ok(())
    }
}
impl embedded_hal::digital::StatefulOutputPin for Output {
    fn is_set_high(&mut self) -> Result<bool, Infallible> {
        Ok(self.0.is_set_high())
    }
    fn is_set_low(&mut self) -> Result<bool, Infallible> {
        Ok(!self.0.is_set_high())
    }
}

impl embedded_hal::digital::ErrorType for Input {
    type Error = Infallible;
}
impl embedded_hal::digital::InputPin for Input {
    fn is_high(&mut self) -> Result<bool, Infallible> {
        Ok(self.0.input_level())
    }
    fn is_low(&mut self) -> Result<bool, Infallible> {
        Ok(!self.0.input_level())
    }
}
