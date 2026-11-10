//! TI INA226 bus voltage / current / power monitor.
//!
//! The current range is the shunt's full scale (81.92 mV across the shunt),
//! so the calibration register is always 2048 and one current LSB is
//! 2.5 µV / R_shunt. With the Stack-chan base's 10 mΩ shunt that is
//! 0.25 mA per LSB and 8.192 A full scale (StackChan-BSP configures
//! 8.19 A). The shunt is given in micro-ohms, like the Linux binding's
//! `shunt-resistor`.
//!
//! Registers are 16-bit, big-endian on the wire (INA226 datasheet, SBOS547).
#![no_std]
#![warn(missing_docs)]

use embedded_hal::i2c::I2c;

/// Manufacturer ID ("TI").
pub const MANUFACTURER_ID: u16 = 0x5449;
/// Calibration for a full-scale current range (see the crate docs).
pub const CALIBRATION: u16 = 2048;
/// Configuration: averages of 16, 1.1 ms conversions, shunt and bus continuous.
pub const CONFIG: u16 = 0x4527;

const REG_CONFIG: u8 = 0x00;
const REG_SHUNT: u8 = 0x01;
const REG_BUS: u8 = 0x02;
const REG_CURRENT: u8 = 0x04;
const REG_CALIBRATION: u8 = 0x05;
const REG_MANUFACTURER: u8 = 0xFE;

/// Driver error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    /// I²C failure.
    I2c(E),
    /// Something else answered at the address.
    WrongDevice(u16),
}

impl<E> From<E> for Error<E> {
    fn from(e: E) -> Self {
        Error::I2c(e)
    }
}

/// The monitor.
pub struct Ina226<I> {
    i2c: I,
    addr: u8,
    shunt_uohm: u32,
}

impl<I: I2c> Ina226<I> {
    /// A driver for the chip at `addr` with a `shunt_uohm` µΩ shunt (no bus traffic).
    pub fn new(i2c: I, addr: u8, shunt_uohm: u32) -> Self {
        Self { i2c, addr, shunt_uohm: shunt_uohm.max(1) }
    }

    fn read(&mut self, r: u8) -> Result<u16, I::Error> {
        let mut b = [0; 2];
        self.i2c.write_read(self.addr, &[r], &mut b)?;
        Ok(u16::from_be_bytes(b))
    }

    fn write(&mut self, r: u8, v: u16) -> Result<(), I::Error> {
        let [h, l] = v.to_be_bytes();
        self.i2c.write(self.addr, &[r, h, l])
    }

    /// Checks the manufacturer ID, then writes the configuration and the calibration.
    pub fn init(&mut self) -> Result<(), Error<I::Error>> {
        let id = self.read(REG_MANUFACTURER)?;
        if id != MANUFACTURER_ID {
            return Err(Error::WrongDevice(id));
        }
        self.write(REG_CONFIG, CONFIG)?;
        self.write(REG_CALIBRATION, CALIBRATION)?;
        Ok(())
    }

    /// Bus voltage in mV (1.25 mV per LSB).
    pub fn bus_mv(&mut self) -> Result<u32, Error<I::Error>> {
        Ok(self.read(REG_BUS)? as u32 * 5 / 4)
    }

    /// Shunt voltage in µV (2.5 µV per LSB, signed).
    pub fn shunt_uv(&mut self) -> Result<i32, Error<I::Error>> {
        Ok(self.read(REG_SHUNT)? as i16 as i32 * 5 / 2)
    }

    /// Current in mA (signed; positive into the shunt's IN+ side).
    pub fn current_ma(&mut self) -> Result<i32, Error<I::Error>> {
        let raw = self.read(REG_CURRENT)? as i16 as i64;
        // LSB = 2.5 µV / R = 2.5 / R_µΩ A = 2500 / R_µΩ mA.
        Ok((raw * 2500 / self.shunt_uohm as i64) as i32)
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::collections::BTreeMap;

    struct Chip(BTreeMap<u8, u16>);

    impl embedded_hal::i2c::ErrorType for Chip {
        type Error = core::convert::Infallible;
    }

    impl I2c for Chip {
        fn transaction(&mut self, _a: u8, ops: &mut [embedded_hal::i2c::Operation<'_>]) -> Result<(), Self::Error> {
            let mut ptr = 0;
            for op in ops {
                match op {
                    embedded_hal::i2c::Operation::Write(b) => {
                        ptr = b[0];
                        if b.len() == 3 {
                            self.0.insert(ptr, u16::from_be_bytes([b[1], b[2]]));
                        }
                    }
                    embedded_hal::i2c::Operation::Read(b) => b.copy_from_slice(&self.0.get(&ptr).copied().unwrap_or(0).to_be_bytes()),
                }
            }
            Ok(())
        }
    }

    #[test]
    fn init_checks_the_id_and_calibrates() {
        let mut m = Ina226::new(Chip(BTreeMap::from([(REG_MANUFACTURER, 0x1234)])), 0x41, 10_000);
        assert_eq!(m.init(), Err(Error::WrongDevice(0x1234)));
        m.i2c.0.insert(REG_MANUFACTURER, MANUFACTURER_ID);
        m.init().unwrap();
        assert_eq!(m.i2c.0[&REG_CALIBRATION], 2048);
        assert_eq!(m.i2c.0[&REG_CONFIG], CONFIG);
    }

    #[test]
    fn scales_follow_the_datasheet() {
        let mut m = Ina226::new(Chip(BTreeMap::new()), 0x41, 10_000);
        m.i2c.0.insert(REG_BUS, 6400); // 8.0 V
        m.i2c.0.insert(REG_CURRENT, 4000); // 4000 × 0.25 mA
        m.i2c.0.insert(REG_SHUNT, (-400i16) as u16);
        assert_eq!(m.bus_mv().unwrap(), 8000);
        assert_eq!(m.current_ma().unwrap(), 1000);
        assert_eq!(m.shunt_uv().unwrap(), -1000);
    }
}
