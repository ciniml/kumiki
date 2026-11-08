//! I²C master, polled. Ported from tab5-os
//! `platform/esp32p4v1/i2c.rs` (after ESP-IDF `esp_hal_i2c` /
//! `esp_driver_i2c`), with embedded-hal's `I2c` on top.
//!
//! One hardware command list is at most a write of up to 30 bytes followed
//! by a read of up to 31 bytes; longer writes and reads are split into
//! several command lists in the same transaction (no STOP in between).

use crate::gpio::{AnyPin, Pull};
use crate::{clear_bits, rd, set_bits, set_field, time, wr};

// Clock and reset (hp_sys_clkrst_reg.h).
const SOC_CLK_CTRL2: usize = 0x500E_601C;
const PERI_CLK_CTRL10: usize = 0x500E_6040;
const HP_RST_EN1: usize = 0x500E_60C4;

/// Controller-specific constants.
#[doc(hidden)]
pub struct Info {
    base: usize,
    sig_sda: u16,
    sig_scl: u16,
    /// APB clock enable in SOC_CLK_CTRL2.
    apb_clk: u32,
    /// Reset in HP_RST_EN1.
    rst: u32,
}

const I2C0_INFO: Info = Info { base: 0x500C_4000, sig_sda: 69, sig_scl: 68, apb_clk: 1 << 12, rst: 1 << 22 };
const I2C1_INFO: Info = Info { base: 0x500C_5000, sig_sda: 71, sig_scl: 70, apb_clk: 1 << 13, rst: 1 << 21 };

/// I²C controller 0 (token).
pub struct I2c0(pub(crate) ());
/// I²C controller 1 (token).
pub struct I2c1(pub(crate) ());

/// An I²C controller token.
pub trait Instance: sealed::Sealed {
    #[doc(hidden)]
    fn info(&self) -> &'static Info;
    #[doc(hidden)]
    fn enable_clock(&self);
}

impl I2c0 {
    /// The controller token without taking [`Peripherals`](crate::Peripherals).
    ///
    /// # Safety
    /// Nothing else may use the controller meanwhile (e.g. a short probe
    /// before the real driver is set up).
    pub unsafe fn steal() -> Self {
        I2c0(())
    }
}

impl I2c1 {
    /// The controller token without taking [`Peripherals`](crate::Peripherals).
    ///
    /// # Safety
    /// See [`I2c0::steal`].
    pub unsafe fn steal() -> Self {
        I2c1(())
    }
}

mod sealed {
    pub trait Sealed {}
    impl Sealed for super::I2c0 {}
    impl Sealed for super::I2c1 {}
}

impl Instance for I2c0 {
    fn info(&self) -> &'static Info {
        &I2C0_INFO
    }
    fn enable_clock(&self) {
        // Source bit 0 (0 = XTAL), enable bit 1, divider bits 2-9.
        clear_bits(PERI_CLK_CTRL10, 1 << 0);
        set_bits(PERI_CLK_CTRL10, 1 << 1);
        set_field(PERI_CLK_CTRL10, 0x3FC, 2, 0);
    }
}

impl Instance for I2c1 {
    fn info(&self) -> &'static Info {
        &I2C1_INFO
    }
    fn enable_clock(&self) {
        // Source bit 26 (0 = XTAL), enable bit 27, divider in CTRL11.
        clear_bits(PERI_CLK_CTRL10, 1 << 26);
        // The divider in CTRL11 is left at its reset value (÷1), as tab5-os does.
        set_bits(PERI_CLK_CTRL10, 1 << 27);
    }
}

// Register offsets (i2c_reg.h; the same on v1 and v3).
const SCL_LOW_PERIOD: usize = 0x00;
const CTR: usize = 0x04;
const SR: usize = 0x08;
const TO: usize = 0x0C;
const FIFO_CONF: usize = 0x18;
const DATA: usize = 0x1C;
const INT_RAW: usize = 0x20;
const INT_CLR: usize = 0x24;
const INT_ENA: usize = 0x28;
const SDA_HOLD: usize = 0x30;
const SDA_SAMPLE: usize = 0x34;
const SCL_HIGH_PERIOD: usize = 0x38;
const SCL_START_HOLD: usize = 0x40;
const SCL_RSTART_SETUP: usize = 0x44;
const SCL_STOP_HOLD: usize = 0x48;
const SCL_STOP_SETUP: usize = 0x4C;
const COMD0: usize = 0x58;
const SCL_SP_CONF: usize = 0x80;

const CTR_MS_MODE: u32 = 1 << 4;
const CTR_TRANS_START: u32 = 1 << 5;
const CTR_FSM_RST: u32 = 1 << 10;
const CTR_CONF_UPGATE: u32 = 1 << 11;
const FIFO_RX_RST: u32 = 1 << 12;
const FIFO_TX_RST: u32 = 1 << 13;
const TO_EN: u32 = 1 << 5;
/// 2^17 cycles of 40 MHz, about 3.3 ms (IDF's value).
const TO_VALUE: u32 = 17;
const SP_RST_SLV_EN: u32 = 1 << 0;
const SR_BUS_BUSY: u32 = 1 << 4;

const INT_TRANS_COMPLETE: u32 = 1 << 7;
/// END_DETECT: a command list ended with END (more to come).
const INT_END_DETECT: u32 = 1 << 3;
const INT_NACK: u32 = 1 << 10;
/// TIME_OUT, ARBITRATION_LOST, SCL_ST_TO, SCL_MAIN_ST_TO.
const INT_ERRORS: u32 = (1 << 8) | (1 << 5) | (1 << 13) | (1 << 14);

// Command words (i2c_ll.h).
const CMD_RSTART: u32 = 6 << 11;
const CMD_WRITE_ACK: u32 = (1 << 11) | (1 << 8);
const CMD_READ: u32 = 3 << 11;
const CMD_READ_NACK: u32 = CMD_READ | (1 << 10);
const CMD_STOP: u32 = 2 << 11;
const CMD_END: u32 = 4 << 11;

const FIFO_SIZE: usize = 32;
/// Software limit on one command list, well above 32 bytes at 100 kHz.
const STEP_US: u64 = 20_000;

/// I²C error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The address was not acknowledged: nobody there.
    AddressNack,
    /// A data byte was not acknowledged.
    DataNack,
    /// Bus error or time-out; the controller was reset and the bus cleared.
    Bus,
}

impl embedded_hal::i2c::Error for Error {
    fn kind(&self) -> embedded_hal::i2c::ErrorKind {
        use embedded_hal::i2c::{ErrorKind, NoAcknowledgeSource};
        match self {
            Error::AddressNack => ErrorKind::NoAcknowledge(NoAcknowledgeSource::Address),
            Error::DataNack => ErrorKind::NoAcknowledge(NoAcknowledgeSource::Data),
            Error::Bus => ErrorKind::Bus,
        }
    }
}

/// I²C master on one controller.
pub struct I2c {
    info: &'static Info,
    _sda: AnyPin,
    _scl: AnyPin,
}

impl I2c {
    /// Sets up the controller at `freq_hz` (100 kHz or 400 kHz typical)
    /// from the 40 MHz crystal, with `sda` / `scl` as open-drain pins.
    pub fn new(inst: impl Instance, sda: impl Into<AnyPin>, scl: impl Into<AnyPin>, freq_hz: u32) -> Self {
        let (sda, scl) = (sda.into(), scl.into());
        let b = inst.info();
        set_bits(SOC_CLK_CTRL2, b.apb_clk);
        inst.enable_clock();
        set_bits(HP_RST_EN1, b.rst);
        clear_bits(HP_RST_EN1, b.rst);
        // Pins: latch high, open drain, GPIO function with input, routed
        // through the matrix both ways.
        for (pin, sig) in [(&sda, b.sig_sda), (&scl, b.sig_scl)] {
            pin.set_level(true);
            pin.configure(Pull::Up, true);
            pin.connect_output(sig);
            pin.connect_input(sig);
            pin.set_output_enable(true);
        }
        // Timing (i2c_ll_master_cal_bus_clk), `half` = source cycles per half period.
        let half = 40_000_000 / freq_hz.max(1) / 2;
        unsafe {
            wr(b.base + CTR, CTR_MS_MODE);
            wr(b.base + FIFO_CONF, 0);
            wr(b.base + SCL_LOW_PERIOD, half - 1);
            wr(b.base + SCL_HIGH_PERIOD, (half / 2 + 2) | ((half / 2 - 2) << 9));
            wr(b.base + SDA_HOLD, half / 4 - 1);
            wr(b.base + SDA_SAMPLE, half / 2 - 1);
            for r in [SCL_START_HOLD, SCL_RSTART_SETUP, SCL_STOP_HOLD, SCL_STOP_SETUP] {
                wr(b.base + r, half - 1);
            }
            wr(b.base + TO, TO_VALUE | TO_EN);
            wr(b.base + INT_ENA, 0);
        }
        let i2c = I2c { info: b, _sda: sda, _scl: scl };
        i2c.update();
        i2c.fifo_reset();
        i2c
    }

    fn update(&self) {
        set_bits(self.info.base + CTR, CTR_CONF_UPGATE);
    }

    fn fifo_reset(&self) {
        let b = self.info.base;
        set_bits(b + FIFO_CONF, FIFO_RX_RST | FIFO_TX_RST);
        clear_bits(b + FIFO_CONF, FIFO_RX_RST | FIFO_TX_RST);
        unsafe { wr(b + INT_CLR, 0x3FFFF) };
    }

    /// After an error: reset the state machine and clock out 9 SCL pulses
    /// so a device stuck mid-byte lets SDA go.
    fn recover(&self) {
        let b = self.info.base;
        set_bits(b + CTR, CTR_FSM_RST);
        unsafe { wr(b + SCL_SP_CONF, (9 << 1) | SP_RST_SLV_EN) };
        self.update();
        let end = time::now() + 2000 * time::TICKS_PER_US;
        while unsafe { rd(b + SCL_SP_CONF) } & SP_RST_SLV_EN != 0 && time::now() < end {}
        clear_bits(b + SCL_SP_CONF, SP_RST_SLV_EN);
        self.update();
        self.fifo_reset();
    }

    /// Runs one command list and waits for TRANS_COMPLETE (or END_DETECT
    /// when the list ends with END). `start` sets TRANS_START; continuing
    /// after END needs it too.
    fn run(&self, cmds: &[u32], ends_with_end: bool, addr_phase: bool) -> Result<(), Error> {
        let b = self.info.base;
        for (i, &c) in cmds.iter().enumerate() {
            unsafe { wr(b + COMD0 + 4 * i, c) };
        }
        self.update();
        set_bits(b + CTR, CTR_TRANS_START);
        let done = if ends_with_end { INT_END_DETECT } else { INT_TRANS_COMPLETE };
        let end = time::now() + STEP_US * time::TICKS_PER_US;
        let r = loop {
            let raw = unsafe { rd(b + INT_RAW) };
            if raw & INT_NACK != 0 {
                break Err(if addr_phase { Error::AddressNack } else { Error::DataNack });
            }
            if raw & INT_ERRORS != 0 || time::now() > end {
                break Err(Error::Bus);
            }
            if raw & done != 0 {
                unsafe { wr(b + INT_CLR, done) };
                break Ok(());
            }
        };
        match r {
            Ok(()) => Ok(()),
            // As ESP-IDF: after a NACK the controller ends the transfer by
            // itself; wait until the bus is free. No reset and no bus clear:
            // 9 SCL pulses after every NACK (an I2C scan) made the GT911
            // stop answering until its next reset (tab5-os doc/17 §9).
            Err(e @ (Error::AddressNack | Error::DataNack)) => {
                let end = time::now() + STEP_US * time::TICKS_PER_US;
                while unsafe { rd(b + SR) } & SR_BUS_BUSY != 0 {
                    if time::now() > end {
                        self.recover();
                        break;
                    }
                }
                self.fifo_reset();
                Err(e)
            }
            Err(e) => {
                self.recover();
                Err(e)
            }
        }
    }

    fn push_fifo(&self, bytes: &[u8]) {
        for &byte in bytes {
            unsafe { wr(self.info.base + DATA, byte as u32) };
        }
    }

    fn pop_fifo(&self, out: &mut [u8]) -> Result<(), Error> {
        let count = (unsafe { rd(self.info.base + SR) } >> 8) as usize & 0x3F;
        if count < out.len() {
            self.recover();
            return Err(Error::Bus);
        }
        for byte in out.iter_mut() {
            *byte = unsafe { rd(self.info.base + DATA) } as u8;
        }
        Ok(())
    }

    /// Runs a transaction: each operation after a (repeated) START with the
    /// address, then STOP.
    fn transaction_impl(&mut self, addr: u8, ops: &mut [embedded_hal::i2c::Operation<'_>]) -> Result<(), Error> {
        use embedded_hal::i2c::Operation;
        self.fifo_reset();
        let last_op = ops.len().saturating_sub(1);
        if ops.is_empty() {
            // Address only: checks for an ACK.
            self.push_fifo(&[addr << 1]);
            return self.run(&[CMD_RSTART, CMD_WRITE_ACK | 1, CMD_STOP], false, true);
        }
        for (i, op) in ops.iter_mut().enumerate() {
            let is_last = i == last_op;
            match op {
                Operation::Write(data) => {
                    // First list: START, address, up to 30 data bytes.
                    let mut first = true;
                    let mut rest: &[u8] = data;
                    loop {
                        let room = if first { FIFO_SIZE - 2 } else { FIFO_SIZE - 1 };
                        let n = rest.len().min(room);
                        let (chunk, tail) = rest.split_at(n);
                        let mut cmds = [0u32; 4];
                        let mut k = 0;
                        if first {
                            self.push_fifo(&[addr << 1]);
                            cmds[k] = CMD_RSTART;
                            cmds[k + 1] = CMD_WRITE_ACK | (1 + n as u32);
                            k += 2;
                        } else if n > 0 {
                            cmds[k] = CMD_WRITE_ACK | n as u32;
                            k += 1;
                        }
                        self.push_fifo(chunk);
                        let done = tail.is_empty();
                        let ends = !(done && is_last);
                        cmds[k] = if ends { CMD_END } else { CMD_STOP };
                        k += 1;
                        self.run(&cmds[..k], ends, first)?;
                        first = false;
                        rest = tail;
                        if done {
                            break;
                        }
                    }
                }
                Operation::Read(buf) => {
                    self.push_fifo(&[(addr << 1) | 1]);
                    let mut first = true;
                    let total = buf.len();
                    let mut off = 0;
                    while off < total || first {
                        let n = (total - off).min(FIFO_SIZE - 1);
                        let last_chunk = off + n == total;
                        let mut cmds = [0u32; 5];
                        let mut k = 0;
                        if first {
                            cmds[k] = CMD_RSTART;
                            cmds[k + 1] = CMD_WRITE_ACK | 1;
                            k += 2;
                        }
                        if last_chunk {
                            if n > 1 {
                                cmds[k] = CMD_READ | (n as u32 - 1);
                                k += 1;
                            }
                            if n > 0 {
                                cmds[k] = CMD_READ_NACK | 1;
                                k += 1;
                            }
                        } else {
                            cmds[k] = CMD_READ | n as u32;
                            k += 1;
                        }
                        let ends = !(last_chunk && is_last);
                        cmds[k] = if ends { CMD_END } else { CMD_STOP };
                        k += 1;
                        self.run(&cmds[..k], ends, first)?;
                        self.pop_fifo(&mut buf[off..off + n])?;
                        first = false;
                        off += n;
                    }
                }
            }
            self.fifo_reset();
        }
        Ok(())
    }
}

impl embedded_hal::i2c::ErrorType for I2c {
    type Error = Error;
}

impl embedded_hal::i2c::I2c for I2c {
    fn transaction(&mut self, address: u8, operations: &mut [embedded_hal::i2c::Operation<'_>]) -> Result<(), Error> {
        self.transaction_impl(address, operations)
    }
}
