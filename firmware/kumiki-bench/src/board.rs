//! Minimal per-board bring-up for the benchmark (replaced by kumiki-boards in M2).
//!
//! Register sequences follow M5GFX `src/M5GFX.cpp` (MIT, (c) 2021 M5Stack) and
//! M5Unified `Power_Class.cpp`; see THIRD_PARTY_NOTICES.md.

use esp_hal::{
    delay::Delay,
    peripherals::{DMA_CH0, Peripherals, SPI2},
};

pub struct Pins {
    pub spi: SPI2<'static>,
    pub dma: DMA_CH0<'static>,
    pub sck: esp_hal::gpio::AnyPin<'static>,
    pub mosi: esp_hal::gpio::AnyPin<'static>,
    pub dc: esp_hal::gpio::AnyPin<'static>,
    pub cs: esp_hal::gpio::AnyPin<'static>,
}

pub struct Parts(Peripherals);

impl Parts {
    pub fn take(p: Peripherals) -> Self {
        Self(p)
    }
}

#[cfg(feature = "cores3")]
mod imp {
    use super::*;
    use esp_hal::i2c::master::{Config as I2cConfig, I2c};
    use esp_hal::time::Rate;
    use kumiki_gfx_panels::{DcsConfig, Ili9342c};

    pub const NAME: &str = "cores3";
    pub const WIDTH: u16 = 320;
    pub const HEIGHT: u16 = 240;
    #[allow(non_upper_case_globals)]
    pub const Model: Ili9342c = Ili9342c;

    const AW9523: u8 = 0x58;
    const AXP2101: u8 = 0x34;

    static mut I2C: Option<I2c<'static, esp_hal::Blocking>> = None;

    fn i2c() -> &'static mut I2c<'static, esp_hal::Blocking> {
        unsafe { (*core::ptr::addr_of_mut!(I2C)).as_mut().unwrap() }
    }

    fn wr(addr: u8, reg: u8, val: u8) {
        i2c().write(addr, &[reg, val]).unwrap();
    }

    pub fn panel_config() -> DcsConfig {
        // CoreS3: ILI9342C, inverted, natural landscape (M5GFX offset_rotation 3 + rotation 1).
        DcsConfig { invert: true, ..DcsConfig::for_model::<Ili9342c>() }
    }

    pub fn power_on(delay: &mut Delay, parts: Parts) -> Pins {
        let p = parts.0;
        let bus = I2c::new(p.I2C0, I2cConfig::default().with_frequency(Rate::from_khz(400)))
            .unwrap()
            .with_sda(p.GPIO12)
            .with_scl(p.GPIO11);
        unsafe { *core::ptr::addr_of_mut!(I2C) = Some(bus) };

        let mut id = [0u8];
        i2c().write_read(AXP2101, &[0x03], &mut id).unwrap();
        esp_println::println!("AXP2101 id=0x{:02X} (expect 0x4A)", id[0]);

        // AW9523B: directions / push-pull / outputs (LCD_RST = P1_5).
        wr(AW9523, 0x04, 0b0001_1000);
        wr(AW9523, 0x05, 0b0000_1100);
        wr(AW9523, 0x11, 0b0001_0000);
        wr(AW9523, 0x02, 0b0000_0111);
        wr(AW9523, 0x03, 0b0000_0011); // LCD_RST low
        // AXP2101: ALDO1..4 + BLDO1 on; voltages (M5Unified board_M5StackCoreS3).
        wr(AXP2101, 0x90, 0xBF);
        wr(AXP2101, 0x92, 18 - 5);
        wr(AXP2101, 0x93, 33 - 5);
        wr(AXP2101, 0x94, 33 - 5);
        wr(AXP2101, 0x95, 33 - 5);
        delay.delay_millis(10);
        wr(AW9523, 0x03, 0b0010_0011); // LCD_RST high
        delay.delay_millis(120);

        Pins {
            spi: p.SPI2,
            dma: p.DMA_CH0,
            sck: p.GPIO36.into(),
            mosi: p.GPIO37.into(),
            dc: p.GPIO35.into(),
            cs: p.GPIO3.into(),
        }
    }

    pub fn backlight_on(_delay: &mut Delay) {
        // DLDO1 = backlight; M5GFX Light_M5StackCoreS3: (brightness + 641) >> 5.
        wr(AXP2101, 0x99, ((200u16 + 641) >> 5) as u8);
        wr(AXP2101, 0x90, 0xBF | 0x80);
    }
}

#[cfg(feature = "atoms3")]
mod imp {
    use super::*;
    use esp_hal::gpio::{Level, Output, OutputConfig};
    use kumiki_gfx_panels::{DcsConfig, Gc9107};

    pub const NAME: &str = "atoms3";
    pub const WIDTH: u16 = 128;
    pub const HEIGHT: u16 = 128;
    #[allow(non_upper_case_globals)]
    pub const Model: Gc9107 = Gc9107;

    static mut BL: Option<Output<'static>> = None;

    pub fn panel_config() -> DcsConfig {
        DcsConfig { panel_size: (128, 128), offset: (0, 32), ..DcsConfig::for_model::<Gc9107>() }
    }

    pub fn power_on(delay: &mut Delay, parts: Parts) -> Pins {
        let p = parts.0;
        let mut rst = Output::new(p.GPIO34, Level::Low, OutputConfig::default());
        delay.delay_millis(10);
        rst.set_high();
        delay.delay_millis(120);
        core::mem::forget(rst); // keep driving high
        let bl = Output::new(p.GPIO16, Level::Low, OutputConfig::default());
        unsafe { *core::ptr::addr_of_mut!(BL) = Some(bl) };
        Pins {
            spi: p.SPI2,
            dma: p.DMA_CH0,
            sck: p.GPIO17.into(),
            mosi: p.GPIO21.into(),
            dc: p.GPIO33.into(),
            cs: p.GPIO15.into(),
        }
    }

    pub fn backlight_on(_delay: &mut Delay) {
        unsafe { (*core::ptr::addr_of_mut!(BL)).as_mut().unwrap().set_high() };
    }
}

pub use imp::*;
