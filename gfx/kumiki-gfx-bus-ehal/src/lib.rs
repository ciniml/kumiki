//! # kumiki-gfx-bus-ehal
//!
//! Portable panel buses built only on `embedded-hal` 1.0 / `embedded-hal-async` 1.0
//! traits. They run on any MCU (RP2040/RP2350, STM32, ESP32, ...). They do not
//! overlap conversion with transfer inside one task; an `async` SPI driver with
//! DMA still frees the CPU for other tasks while each chunk is on the wire.
//!
//! For the ESP32 family, `kumiki-gfx-bus-esp` adds a DMA double-buffered bus.
#![no_std]
#![warn(missing_docs)]

use embedded_hal::digital::OutputPin;
use kumiki_gfx::bus::{asynch, blocking, ChunkPlan, ErrorType};

/// Bus error.
#[derive(Debug)]
pub enum Error<S, P> {
    /// SPI error.
    Spi(S),
    /// D/C pin error.
    Pin(P),
}

/// MIPI-DCS over a blocking [`embedded_hal::spi::SpiDevice`] plus a D/C pin.
///
/// `buf` is the scratch buffer used for pixel streaming (larger = fewer SPI
/// transactions; 1–4 KiB is typical).
pub struct SpiDeviceBus<'b, SPI, DC> {
    spi: SPI,
    dc: DC,
    buf: &'b mut [u8],
}

impl<'b, SPI, DC> SpiDeviceBus<'b, SPI, DC> {
    /// Create a bus. `buf` must not be empty.
    pub fn new(spi: SPI, dc: DC, buf: &'b mut [u8]) -> Self {
        assert!(!buf.is_empty());
        Self { spi, dc, buf }
    }
    /// Release the parts.
    pub fn release(self) -> (SPI, DC, &'b mut [u8]) {
        (self.spi, self.dc, self.buf)
    }
}

impl<SPI: embedded_hal::spi::SpiDevice, DC: OutputPin> ErrorType for SpiDeviceBus<'_, SPI, DC> {
    type Error = Error<SPI::Error, DC::Error>;
}

impl<SPI: embedded_hal::spi::SpiDevice, DC: OutputPin> blocking::PanelBus for SpiDeviceBus<'_, SPI, DC> {
    fn command(&mut self, cmd: u8, params: &[u8]) -> Result<(), Self::Error> {
        self.dc.set_low().map_err(Error::Pin)?;
        self.spi.write(&[cmd]).map_err(Error::Spi)?;
        self.dc.set_high().map_err(Error::Pin)?;
        if !params.is_empty() {
            self.spi.write(params).map_err(Error::Spi)?;
        }
        Ok(())
    }

    fn write_data(&mut self, data: &[u8]) -> Result<(), Self::Error> {
        self.spi.write(data).map_err(Error::Spi) // no copy needed
    }

    fn write_with<F: FnMut(&mut [u8])>(&mut self, total: usize, unit: usize, mut fill: F) -> Result<(), Self::Error> {
        for n in ChunkPlan::new(total, unit, self.buf.len()) {
            fill(&mut self.buf[..n]);
            self.spi.write(&self.buf[..n]).map_err(Error::Spi)?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// MIPI-DCS over an `async` [`embedded_hal_async::spi::SpiDevice`] plus a D/C pin.
pub struct AsyncSpiDeviceBus<'b, SPI, DC> {
    spi: SPI,
    dc: DC,
    buf: &'b mut [u8],
}

impl<'b, SPI, DC> AsyncSpiDeviceBus<'b, SPI, DC> {
    /// Create a bus. `buf` must not be empty.
    pub fn new(spi: SPI, dc: DC, buf: &'b mut [u8]) -> Self {
        assert!(!buf.is_empty());
        Self { spi, dc, buf }
    }
    /// Release the parts.
    pub fn release(self) -> (SPI, DC, &'b mut [u8]) {
        (self.spi, self.dc, self.buf)
    }
}

impl<SPI: embedded_hal_async::spi::SpiDevice, DC: OutputPin> ErrorType for AsyncSpiDeviceBus<'_, SPI, DC> {
    type Error = Error<SPI::Error, DC::Error>;
}

impl<SPI: embedded_hal_async::spi::SpiDevice, DC: OutputPin> asynch::PanelBus for AsyncSpiDeviceBus<'_, SPI, DC> {
    async fn command(&mut self, cmd: u8, params: &[u8]) -> Result<(), Self::Error> {
        self.dc.set_low().map_err(Error::Pin)?;
        self.spi.write(&[cmd]).await.map_err(Error::Spi)?;
        self.dc.set_high().map_err(Error::Pin)?;
        if !params.is_empty() {
            self.spi.write(params).await.map_err(Error::Spi)?;
        }
        Ok(())
    }

    async fn write_data(&mut self, data: &[u8]) -> Result<(), Self::Error> {
        self.spi.write(data).await.map_err(Error::Spi)
    }

    async fn write_with<F: FnMut(&mut [u8])>(&mut self, total: usize, unit: usize, mut fill: F) -> Result<(), Self::Error> {
        for n in ChunkPlan::new(total, unit, self.buf.len()) {
            fill(&mut self.buf[..n]);
            self.spi.write(&self.buf[..n]).await.map_err(Error::Spi)?;
        }
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
