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
//! With the `fifo-esp32s3` feature and [`EspSpiDmaBus::with_fifo`], transfers of
//! at most 64 bytes (commands, window setup, short pixel runs) bypass DMA and go
//! through the SPI data registers, as M5GFX does. This removes the per-transfer
//! DMA setup cost that dominates small drawing operations.
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
/// Largest transfer sent through the FIFO fast path.
pub const FIFO_MAX: usize = 64;

/// SPI host used by the bus (needed for the FIFO fast path).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpiHost {
    /// SPI2 (FSPI)
    Spi2,
    /// SPI3
    Spi3,
}

#[cfg(feature = "fifo-esp32s3")]
mod fifo {
    use super::SpiHost;
    use esp_hal::clock::ll::{ClockTree, SpiInstance};
    use esp32s3::spi2::RegisterBlock;

    pub(crate) struct Fifo {
        host: SpiHost,
        /// Registers are currently set up for CPU (FIFO) transfers.
        armed: core::cell::Cell<bool>,
    }

    impl Fifo {
        pub(crate) fn new(host: SpiHost) -> Self {
            // Keep the SPI function clock enabled while the bus lives, so the
            // data registers work between esp-hal DMA transfers (esp-hal only
            // requests the clock for the duration of each transfer).
            ClockTree::with(|c| instance(host).request_function_clock(c));
            Self { host, armed: core::cell::Cell::new(false) }
        }

        /// esp-hal reconfigures the registers for DMA on every DMA transfer.
        #[inline]
        pub(crate) fn disarm(&self) {
            self.armed.set(false);
        }

        fn regs(&self) -> &'static RegisterBlock {
            unsafe {
                &*match self.host {
                    SpiHost::Spi2 => esp32s3::SPI2::ptr(),
                    SpiHost::Spi3 => esp32s3::SPI3::ptr(),
                }
            }
        }

        #[inline]
        pub(crate) fn wait(&self) {
            let r = self.regs();
            while r.cmd().read().usr().bit_is_set() {}
        }

        /// Start a TX-only transfer of `data` (1..=64 bytes). Does not wait.
        #[inline]
        pub(crate) fn write(&self, data: &[u8]) {
            let r = self.regs();
            self.wait();
            if !self.armed.get() {
                r.dma_conf().modify(|_, w| w.dma_tx_ena().clear_bit().dma_rx_ena().clear_bit());
                r.user().modify(|_, w| w.usr_mosi().set_bit().usr_miso().clear_bit());
                self.armed.set(true);
            }
            r.ms_dlen().write(|w| unsafe { w.ms_data_bitlen().bits((data.len() * 8 - 1) as u32) });
            for (i, chunk) in data.chunks(4).enumerate() {
                let mut b = [0u8; 4];
                b[..chunk.len()].copy_from_slice(chunk);
                r.w(i).write(|w| unsafe { w.bits(u32::from_le_bytes(b)) });
            }
            // UPDATE and USR in one write (M5GFX `SPI_EXECUTE` on ESP32-S3).
            r.cmd().write(|w| w.update().set_bit().usr().set_bit());
        }
    }

    impl Drop for Fifo {
        fn drop(&mut self) {
            self.wait();
            ClockTree::with(|c| instance(self.host).release_function_clock(c));
        }
    }

    fn instance(host: SpiHost) -> SpiInstance {
        match host {
            SpiHost::Spi2 => SpiInstance::Spi2,
            SpiHost::Spi3 => SpiInstance::Spi3,
        }
    }
}

#[cfg(not(feature = "fifo-esp32s3"))]
mod fifo {
    use super::SpiHost;
    pub(crate) enum Fifo {}
    impl Fifo {
        pub(crate) fn new(_host: SpiHost) -> Self {
            panic!("enable the `fifo-esp32s3` feature of kumiki-gfx-bus-esp")
        }
        pub(crate) fn wait(&self) {
            match *self {}
        }
        pub(crate) fn write(&self, _data: &[u8]) {
            match *self {}
        }
        pub(crate) fn disarm(&self) {
            match *self {}
        }
    }
}

use fifo::Fifo;

mod qspi;
pub use qspi::EspQspiDmaBus;

pub(crate) enum Link<'d, Dm: DriverMode> {
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

/// Lets frame-buffered surfaces (which cannot fail) share this error type.
impl From<core::convert::Infallible> for Error {
    fn from(e: core::convert::Infallible) -> Self {
        match e {}
    }
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
    fifo: Option<Fifo>,
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
        Self { link: Link::Idle(spi), free: [Some(buf_a), Some(buf_b)], dc, _cs: cs, cap, fifo: None }
    }

    /// Enable the FIFO fast path for transfers of at most [`FIFO_MAX`] bytes.
    /// `host` must be the SPI host `spi` was created from.
    /// Requires the `fifo-esp32s3` feature (panics otherwise).
    pub fn with_fifo(mut self, host: SpiHost) -> Self {
        self.fifo = Some(Fifo::new(host));
        self
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

    fn wait_fifo(&self) {
        if let Some(f) = &self.fifo {
            f.wait();
        }
    }

    /// Start transmitting `len` bytes of `buf` by DMA. The DMA link must be idle.
    fn start(&mut self, mut buf: DmaTxBuf, len: usize) -> Result<(), Error> {
        self.wait_fifo();
        if let Some(f) = &self.fifo {
            f.disarm();
        }
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

    /// Blocking wait for the in-flight DMA transfer (any driver mode).
    fn wait_dma_blocking(&mut self) {
        if let Link::Busy(_) = self.link {
            let Link::Busy(xfer) = core::mem::replace(&mut self.link, Link::Poisoned) else { unreachable!() };
            let (spi, buf) = xfer.wait();
            self.link = Link::Idle(spi);
            self.put_free(buf);
        }
    }

    /// Wait until nothing is on the wire (DMA and FIFO).
    fn wait_all_blocking(&mut self) {
        self.wait_dma_blocking();
        self.wait_fifo();
    }

    /// Send a small block: FIFO if available, otherwise a DMA buffer. The link
    /// must be idle. Leaves the transfer in flight.
    fn send_small_now(&mut self, bytes: &[u8]) -> Result<(), Error> {
        if let (Some(f), true) = (&self.fifo, bytes.len() <= FIFO_MAX) {
            f.write(bytes);
            return Ok(());
        }
        let mut buf = self.take_free().ok_or(Error::Poisoned)?;
        buf.as_mut_slice()[..bytes.len()].copy_from_slice(bytes);
        self.start(buf, bytes.len())
    }

    fn rebuild<Dm2: DriverMode>(mut self, f: impl FnOnce(SpiDma<'d, Dm>) -> SpiDma<'d, Dm2>) -> EspSpiDmaBus<'d, Dm2> {
        self.wait_all_blocking();
        let Link::Idle(spi) = core::mem::replace(&mut self.link, Link::Poisoned) else {
            panic!("bus poisoned")
        };
        EspSpiDmaBus { link: Link::Idle(f(spi)), free: self.free, dc: self.dc, _cs: self._cs, cap: self.cap, fifo: self.fifo }
    }
}

impl<'d, Dm: DriverMode> ErrorType for EspSpiDmaBus<'d, Dm> {
    type Error = Error;
}

// ---- blocking --------------------------------------------------------------

impl<'d> EspSpiDmaBus<'d, Blocking> {
    /// Switch to `async` mode (waits for any in-flight transfer first).
    pub fn into_async(self) -> EspSpiDmaBus<'d, Async> {
        self.rebuild(|s| s.into_async())
    }
}

impl<'d> blocking::PanelBus for EspSpiDmaBus<'d, Blocking> {
    fn command(&mut self, cmd: u8, params: &[u8]) -> Result<(), Error> {
        self.wait_all_blocking();
        self.dc.set_low();
        self.send_small_now(&[cmd])?;
        self.wait_all_blocking();
        self.dc.set_high();
        if !params.is_empty() {
            // Left in flight: the next command waits for it before lowering D/C.
            self.send_small_now(params)?;
        }
        Ok(())
    }

    fn write_with<F: FnMut(&mut [u8])>(&mut self, total: usize, unit: usize, mut fill: F) -> Result<(), Error> {
        if self.fifo.is_some() && total <= FIFO_MAX {
            let mut tmp = [0u8; FIFO_MAX];
            fill(&mut tmp[..total]);
            self.wait_dma_blocking();
            return self.send_small_now(&tmp[..total]);
        }
        for n in ChunkPlan::new(total, unit, self.cap) {
            // A free buffer always exists: at most one is in flight.
            let mut buf = self.take_free().ok_or(Error::Poisoned)?;
            fill(&mut buf.as_mut_slice()[..n]); // overlaps with the in-flight chunk
            self.wait_dma_blocking();
            self.start(buf, n)?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Error> {
        self.wait_all_blocking();
        Ok(())
    }
}

// ---- async -----------------------------------------------------------------

impl<'d> EspSpiDmaBus<'d, Async> {
    async fn wait_dma(&mut self) {
        if let Link::Busy(xfer) = &mut self.link {
            xfer.wait_for_done().await;
        }
        self.wait_dma_blocking(); // returns immediately: already done
    }

    async fn wait_all(&mut self) {
        self.wait_dma().await;
        self.wait_fifo(); // at most 64 bytes: a few µs
    }

    /// Switch back to blocking mode.
    pub fn into_blocking(self) -> EspSpiDmaBus<'d, Blocking> {
        self.rebuild(|s| s.into_blocking())
    }
}

impl<'d> asynch::PanelBus for EspSpiDmaBus<'d, Async> {
    async fn command(&mut self, cmd: u8, params: &[u8]) -> Result<(), Error> {
        self.wait_all().await;
        self.dc.set_low();
        self.send_small_now(&[cmd])?;
        self.wait_all().await;
        self.dc.set_high();
        if !params.is_empty() {
            self.send_small_now(params)?;
        }
        Ok(())
    }

    async fn write_with<F: FnMut(&mut [u8])>(&mut self, total: usize, unit: usize, mut fill: F) -> Result<(), Error> {
        if self.fifo.is_some() && total <= FIFO_MAX {
            let mut tmp = [0u8; FIFO_MAX];
            fill(&mut tmp[..total]);
            self.wait_dma().await;
            return self.send_small_now(&tmp[..total]);
        }
        for n in ChunkPlan::new(total, unit, self.cap) {
            let mut buf = self.take_free().ok_or(Error::Poisoned)?;
            fill(&mut buf.as_mut_slice()[..n]);
            self.wait_dma().await;
            self.start(buf, n)?;
        }
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), Error> {
        self.wait_all().await;
        Ok(())
    }
}
