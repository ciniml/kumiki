//! # kumiki-gfx-bus-esp
//!
//! ESP32-family panel buses for [`kumiki_gfx`].
//!
//! [`EspSpiDmaBus`] drives a MIPI-DCS panel over esp-hal's `SpiDma` with **two
//! DMA buffers**: while buffer A is on the wire, the CPU fills (converts into)
//! buffer B; then A is awaited and B started. This is the Rust counterpart of
//! M5GFX's `Bus_SPI::writePixels` flip-buffer pipeline, expressed with
//! ownership (esp-hal moves the bus and the buffer into the transfer object, so
//! a buffer can never be touched while DMA reads it).
//!
//! Works in both esp-hal driver modes: `Blocking` implements the blocking
//! [`PanelBus`](kumiki_gfx::bus::blocking::PanelBus), `Async` the `async` one
//! (executor-agnostic; completion is interrupt/waker driven inside esp-hal).
#![no_std]
#![warn(missing_docs)]

use esp_hal::{
    Async, Blocking, DriverMode,
    dma::DmaTxBuf,
    gpio::Output,
    spi::{
        Error as SpiError,
        master::{SpiDma, SpiDmaTransfer},
    },
};
use kumiki_gfx::bus::{ChunkPlan, ErrorType, asynch, blocking};

/// Largest single `write_buffer` transfer supported by esp-hal's SPI DMA.
pub const MAX_TRANSFER: usize = 32736;

enum Link<'d, Dm: DriverMode> {
    Idle(SpiDma<'d, Dm>),
    Busy(SpiDmaTransfer<'d, Dm, DmaTxBuf>),
    Poisoned,
}

/// Bus error.
#[derive(Debug)]
pub enum Error {
    /// esp-hal SPI error.
    Spi(SpiError),
    /// The bus was left unusable by an earlier failure.
    Poisoned,
}

/// SPI + DMA panel bus with double-buffered pipelining.
///
/// Owns D/C and (optionally) CS. CS, when given, is held asserted for the
/// lifetime of the bus (the panel is assumed to own the SPI host, as on
/// M5Stack CoreS3 / AtomS3).
pub struct EspSpiDmaBus<'d, Dm: DriverMode> {
    link: Link<'d, Dm>,
    free: [Option<DmaTxBuf>; 2],
    dc: Output<'d>,
    _cs: Option<Output<'d>>,
    cap: usize,
}

impl<'d, Dm: DriverMode> EspSpiDmaBus<'d, Dm> {
    /// Create a bus from a DMA-enabled SPI master and two TX buffers
    /// (e.g. `dma_tx_buffer!(16384)`). Both buffers should be the same size.
    pub fn new(spi: SpiDma<'d, Dm>, dc: Output<'d>, cs: Option<Output<'d>>, buf_a: DmaTxBuf, buf_b: DmaTxBuf) -> Self {
        let cap = buf_a.capacity().min(buf_b.capacity()).min(MAX_TRANSFER);
        let mut cs = cs;
        if let Some(cs) = cs.as_mut() {
            cs.set_low();
        }
        Self { link: Link::Idle(spi), free: [Some(buf_a), Some(buf_b)], dc, _cs: cs, cap }
    }

    /// Scratch capacity per DMA chunk.
    pub fn chunk_capacity(&self) -> usize {
        self.cap
    }

    fn put_free(&mut self, buf: DmaTxBuf) {
        if self.free[0].is_none() {
            self.free[0] = Some(buf);
        } else {
            self.free[1] = Some(buf);
        }
    }

    fn take_free(&mut self) -> Option<DmaTxBuf> {
        self.free[0].take().or_else(|| self.free[1].take())
    }

    /// Start transmitting `len` bytes of `buf`. The link must be idle.
    fn start(&mut self, mut buf: DmaTxBuf, len: usize) -> Result<(), Error> {
        let Link::Idle(spi) = core::mem::replace(&mut self.link, Link::Poisoned) else {
            self.put_free(buf);
            return Err(Error::Poisoned);
        };
        buf.set_length(len);
        match spi.write_buffer(len, buf) {
            Ok(xfer) => {
                self.link = Link::Busy(xfer);
                Ok(())
            }
            Err((e, spi, buf)) => {
                self.link = Link::Idle(spi);
                self.put_free(buf);
                Err(Error::Spi(e))
            }
        }
    }

    /// Blocking wait for the in-flight transfer (any driver mode).
    fn wait_idle_blocking(&mut self) {
        if let Link::Busy(_) = self.link {
            let Link::Busy(xfer) = core::mem::replace(&mut self.link, Link::Poisoned) else { unreachable!() };
            let (spi, buf) = xfer.wait();
            self.link = Link::Idle(spi);
            self.put_free(buf);
        }
    }
}

impl<'d, Dm: DriverMode> ErrorType for EspSpiDmaBus<'d, Dm> {
    type Error = Error;
}

// ---- blocking --------------------------------------------------------------

impl<'d> EspSpiDmaBus<'d, Blocking> {
    /// Switch to `async` mode (waits for any in-flight transfer first).
    pub fn into_async(mut self) -> EspSpiDmaBus<'d, Async> {
        self.wait_idle_blocking();
        let Link::Idle(spi) = core::mem::replace(&mut self.link, Link::Poisoned) else {
            panic!("bus poisoned")
        };
        EspSpiDmaBus { link: Link::Idle(spi.into_async()), free: self.free, dc: self.dc, _cs: self._cs, cap: self.cap }
    }

    fn send_small(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.wait_idle_blocking();
        let mut buf = self.take_free().ok_or(Error::Poisoned)?;
        buf.as_mut_slice()[..bytes.len()].copy_from_slice(bytes);
        self.start(buf, bytes.len())
    }
}

impl<'d> blocking::PanelBus for EspSpiDmaBus<'d, Blocking> {
    fn command(&mut self, cmd: u8, params: &[u8]) -> Result<(), Error> {
        self.wait_idle_blocking();
        self.dc.set_low();
        self.send_small(&[cmd])?;
        self.wait_idle_blocking();
        self.dc.set_high();
        if !params.is_empty() {
            // Left in flight: the next command waits for it before lowering D/C.
            self.send_small(params)?;
        }
        Ok(())
    }

    fn write_with<F: FnMut(&mut [u8])>(&mut self, total: usize, unit: usize, mut fill: F) -> Result<(), Error> {
        for n in ChunkPlan::new(total, unit, self.cap) {
            // A free buffer always exists: at most one is in flight.
            let mut buf = self.take_free().ok_or(Error::Poisoned)?;
            fill(&mut buf.as_mut_slice()[..n]); // overlaps with the in-flight chunk
            self.wait_idle_blocking();
            self.start(buf, n)?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Error> {
        self.wait_idle_blocking();
        Ok(())
    }
}

// ---- async -----------------------------------------------------------------

impl<'d> EspSpiDmaBus<'d, Async> {
    async fn wait_idle(&mut self) {
        if let Link::Busy(xfer) = &mut self.link {
            xfer.wait_for_done().await;
        }
        self.wait_idle_blocking(); // returns immediately: already done
    }

    async fn send_small(&mut self, bytes: &[u8]) -> Result<(), Error> {
        self.wait_idle().await;
        let mut buf = self.take_free().ok_or(Error::Poisoned)?;
        buf.as_mut_slice()[..bytes.len()].copy_from_slice(bytes);
        self.start(buf, bytes.len())
    }

    /// Switch back to blocking mode.
    pub fn into_blocking(mut self) -> EspSpiDmaBus<'d, Blocking> {
        self.wait_idle_blocking();
        let Link::Idle(spi) = core::mem::replace(&mut self.link, Link::Poisoned) else {
            panic!("bus poisoned")
        };
        EspSpiDmaBus { link: Link::Idle(spi.into_blocking()), free: self.free, dc: self.dc, _cs: self._cs, cap: self.cap }
    }
}

impl<'d> asynch::PanelBus for EspSpiDmaBus<'d, Async> {
    async fn command(&mut self, cmd: u8, params: &[u8]) -> Result<(), Error> {
        self.wait_idle().await;
        self.dc.set_low();
        self.send_small(&[cmd]).await?;
        self.wait_idle().await;
        self.dc.set_high();
        if !params.is_empty() {
            self.send_small(params).await?;
        }
        Ok(())
    }

    async fn write_with<F: FnMut(&mut [u8])>(&mut self, total: usize, unit: usize, mut fill: F) -> Result<(), Error> {
        for n in ChunkPlan::new(total, unit, self.cap) {
            let mut buf = self.take_free().ok_or(Error::Poisoned)?;
            fill(&mut buf.as_mut_slice()[..n]);
            self.wait_idle().await;
            self.start(buf, n)?;
        }
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), Error> {
        self.wait_idle().await;
        Ok(())
    }
}
