//! QSPI panel bus (AMOLED controllers such as CO5300, SH8601, RM67162).
//!
//! These controllers frame every SPI transaction as
//! `opcode (8 bit, 1 line) + address (24 bit, 1 line) + data`:
//!
//! * register write: opcode `0x02`, address `cmd << 8`, parameters on 1 line;
//! * pixel write: opcode `0x32`, address `0x2C << 8` (RAMWR) for the first
//!   chunk and `0x3C << 8` (write memory continue) for the following ones,
//!   pixel data on 4 lines.
//!
//! Each transaction toggles the hardware CS, so the pixel stream can be cut
//! into DMA-sized chunks and pipelined exactly like [`crate::EspSpiDmaBus`].
//! The panel driver stays the generic MIPI-DCS [`kumiki_gfx_panels`]-style
//! one: the bus turns its `RAMWR` command into the start of a quad stream.

use esp_hal::{
    Async, Blocking, DriverMode,
    dma::DmaTxBuf,
    spi::master::{Address, Command, DataMode, SpiDma},
};
use kumiki_gfx::bus::{ChunkPlan, ErrorType, asynch, blocking};

use crate::{Error, Link, MAX_TRANSFER};

const OP_WRITE_REG: u16 = 0x02;
const OP_WRITE_PIXELS_QUAD: u16 = 0x32;
const RAMWR: u8 = 0x2C;
const RAMWRC: u8 = 0x3C;

/// QSPI + DMA panel bus with double-buffered pipelining.
///
/// The SPI master must be configured with SCK, CS (hardware, toggled per
/// transaction) and SIO0..SIO3.
pub struct EspQspiDmaBus<'d, Dm: DriverMode> {
    link: Link<'d, Dm>,
    free: [Option<DmaTxBuf>; 2],
    cap: usize,
    /// Next pixel chunk is the first one after RAMWR.
    stream_start: bool,
}

impl<'d, Dm: DriverMode> EspQspiDmaBus<'d, Dm> {
    /// Create a bus from a DMA-enabled SPI master and two TX buffers.
    pub fn new(spi: SpiDma<'d, Dm>, buf_a: DmaTxBuf, buf_b: DmaTxBuf) -> Self {
        let cap = buf_a.capacity().min(buf_b.capacity()).min(MAX_TRANSFER);
        Self { link: Link::Idle(spi), free: [Some(buf_a), Some(buf_b)], cap, stream_start: false }
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

    /// Start one framed transaction carrying `len` bytes of `buf`.
    fn start(&mut self, op: u16, reg: u8, mode: DataMode, mut buf: DmaTxBuf, len: usize) -> Result<(), Error> {
        let Link::Idle(spi) = core::mem::replace(&mut self.link, Link::Poisoned) else {
            self.put_free(buf);
            return Err(Error::Poisoned);
        };
        buf.set_length(len);
        let cmd = Command::_8Bit(op, DataMode::Single);
        let addr = Address::_24Bit((reg as u32) << 8, DataMode::Single);
        match spi.half_duplex_write_buffer(mode, cmd, addr, 0, len, buf) {
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

    fn wait_blocking(&mut self) {
        if let Link::Busy(_) = self.link {
            let Link::Busy(xfer) = core::mem::replace(&mut self.link, Link::Poisoned) else { unreachable!() };
            let (spi, buf) = xfer.wait();
            self.link = Link::Idle(spi);
            self.put_free(buf);
        }
    }

    /// Register write (opcode 0x02). The link must be idle.
    fn send_command(&mut self, cmd: u8, params: &[u8]) -> Result<(), Error> {
        let mut buf = self.take_free().ok_or(Error::Poisoned)?;
        buf.as_mut_slice()[..params.len()].copy_from_slice(params);
        self.start(OP_WRITE_REG, cmd, DataMode::Single, buf, params.len())
    }

    fn pixel_reg(&mut self) -> u8 {
        if core::mem::replace(&mut self.stream_start, false) { RAMWR } else { RAMWRC }
    }
}

impl<'d, Dm: DriverMode> ErrorType for EspQspiDmaBus<'d, Dm> {
    type Error = Error;
}

impl<'d> EspQspiDmaBus<'d, Blocking> {
    /// Switch to `async` mode.
    pub fn into_async(mut self) -> EspQspiDmaBus<'d, Async> {
        self.wait_blocking();
        let Link::Idle(spi) = core::mem::replace(&mut self.link, Link::Poisoned) else { panic!("bus poisoned") };
        EspQspiDmaBus { link: Link::Idle(spi.into_async()), free: self.free, cap: self.cap, stream_start: self.stream_start }
    }
}

impl<'d> blocking::PanelBus for EspQspiDmaBus<'d, Blocking> {
    fn command(&mut self, cmd: u8, params: &[u8]) -> Result<(), Error> {
        self.wait_blocking();
        if cmd == RAMWR && params.is_empty() {
            self.stream_start = true; // the opcode is sent with the first pixel chunk
            return Ok(());
        }
        self.send_command(cmd, params)
    }

    fn write_with<F: FnMut(&mut [u8])>(&mut self, total: usize, unit: usize, mut fill: F) -> Result<(), Error> {
        for n in ChunkPlan::new(total, unit, self.cap) {
            let mut buf = self.take_free().ok_or(Error::Poisoned)?;
            fill(&mut buf.as_mut_slice()[..n]); // overlaps with the in-flight chunk
            self.wait_blocking();
            let reg = self.pixel_reg();
            self.start(OP_WRITE_PIXELS_QUAD, reg, DataMode::Quad, buf, n)?;
        }
        Ok(())
    }

    fn flush(&mut self) -> Result<(), Error> {
        self.wait_blocking();
        Ok(())
    }
}

impl<'d> EspQspiDmaBus<'d, Async> {
    async fn wait(&mut self) {
        if let Link::Busy(xfer) = &mut self.link {
            xfer.wait_for_done().await;
        }
        self.wait_blocking();
    }

    /// Switch back to blocking mode.
    pub fn into_blocking(mut self) -> EspQspiDmaBus<'d, Blocking> {
        self.wait_blocking();
        let Link::Idle(spi) = core::mem::replace(&mut self.link, Link::Poisoned) else { panic!("bus poisoned") };
        EspQspiDmaBus { link: Link::Idle(spi.into_blocking()), free: self.free, cap: self.cap, stream_start: self.stream_start }
    }
}

impl<'d> asynch::PanelBus for EspQspiDmaBus<'d, Async> {
    async fn command(&mut self, cmd: u8, params: &[u8]) -> Result<(), Error> {
        self.wait().await;
        if cmd == RAMWR && params.is_empty() {
            self.stream_start = true;
            return Ok(());
        }
        self.send_command(cmd, params)
    }

    async fn write_with<F: FnMut(&mut [u8])>(&mut self, total: usize, unit: usize, mut fill: F) -> Result<(), Error> {
        for n in ChunkPlan::new(total, unit, self.cap) {
            let mut buf = self.take_free().ok_or(Error::Poisoned)?;
            fill(&mut buf.as_mut_slice()[..n]);
            self.wait().await;
            let reg = self.pixel_reg();
            self.start(OP_WRITE_PIXELS_QUAD, reg, DataMode::Quad, buf, n)?;
        }
        Ok(())
    }

    async fn flush(&mut self) -> Result<(), Error> {
        self.wait().await;
        Ok(())
    }
}
