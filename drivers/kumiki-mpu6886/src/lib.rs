//! # kumiki-mpu6886
//!
//! Driver for the TDK InvenSense MPU-6886 6-axis IMU (M5Stack AtomS3, Core2,
//! StickC Plus2, ...), on `embedded-hal` 1.0 I²C (blocking) and
//! `embedded-hal-async` 1.0 I²C (`async`).
//!
//! Configuration follows M5Unified's `MPU6886_Class::begin`: ±8 g, ±2000 dps,
//! DLPF 176 Hz, 1 kHz / (1 + 5) sample rate.
#![no_std]
#![warn(missing_docs)]

use embedded_hal::{delay::DelayNs, i2c::I2c};

/// Default 7-bit I²C address (AD0 low).
pub const DEFAULT_ADDRESS: u8 = 0x68;
/// WHO_AM_I value of the MPU-6886.
pub const WHO_AM_I_VALUE: u8 = 0x19;

const REG_SMPLRT_DIV: u8 = 0x19;
const REG_CONFIG: u8 = 0x1A;
const REG_GYRO_CONFIG: u8 = 0x1B;
const REG_ACCEL_CONFIG: u8 = 0x1C;
const REG_ACCEL_CONFIG2: u8 = 0x1D;
const REG_FIFO_EN: u8 = 0x23;
const REG_INT_PIN_CFG: u8 = 0x37;
const REG_INT_ENABLE: u8 = 0x38;
const REG_ACCEL_XOUT_H: u8 = 0x3B;
const REG_GYRO_XOUT_H: u8 = 0x43;
const REG_USER_CTRL: u8 = 0x6A;
const REG_PWR_MGMT_1: u8 = 0x6B;
const REG_WHO_AM_I: u8 = 0x75;

/// LSB per g at ±8 g.
const ACCEL_LSB_PER_G: f32 = 4096.0;
/// LSB per degree/s at ±2000 dps.
const GYRO_LSB_PER_DPS: f32 = 16.4;

/// Driver error.
#[derive(Debug)]
pub enum Error<E> {
    /// I²C error.
    I2c(E),
    /// WHO_AM_I did not match.
    WrongId(u8),
}

/// Raw 16-bit samples.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Raw3 {
    /// X
    pub x: i16,
    /// Y
    pub y: i16,
    /// Z
    pub z: i16,
}

/// MPU-6886 driver.
pub struct Mpu6886<I> {
    i2c: I,
    addr: u8,
}

const INIT_SEQUENCE: [(u8, u8, u32); 13] = [
    (REG_PWR_MGMT_1, 0x00, 10),
    (REG_PWR_MGMT_1, 0x80, 10), // device reset
    (REG_PWR_MGMT_1, 0x01, 10), // auto-select clock
    (REG_ACCEL_CONFIG, 0x10, 1), // ±8 g
    (REG_GYRO_CONFIG, 0x18, 1),  // ±2000 dps
    (REG_CONFIG, 0x01, 1),       // DLPF 176 Hz
    (REG_SMPLRT_DIV, 0x05, 1),
    (REG_INT_ENABLE, 0x00, 1),
    (REG_ACCEL_CONFIG2, 0x00, 1),
    (REG_USER_CTRL, 0x00, 1),
    (REG_FIFO_EN, 0x00, 1),
    (REG_INT_PIN_CFG, 0x22, 1),
    (REG_INT_ENABLE, 0x01, 1),
];

fn be3(b: &[u8; 6]) -> Raw3 {
    Raw3 {
        x: i16::from_be_bytes([b[0], b[1]]),
        y: i16::from_be_bytes([b[2], b[3]]),
        z: i16::from_be_bytes([b[4], b[5]]),
    }
}

impl<I> Mpu6886<I> {
    /// Create a driver (does not touch the bus).
    pub fn new(i2c: I, addr: u8) -> Self {
        Self { i2c, addr }
    }
    /// Release the bus.
    pub fn release(self) -> I {
        self.i2c
    }
    /// Convert raw acceleration to g.
    pub fn accel_g(raw: Raw3) -> [f32; 3] {
        [raw.x as f32 / ACCEL_LSB_PER_G, raw.y as f32 / ACCEL_LSB_PER_G, raw.z as f32 / ACCEL_LSB_PER_G]
    }
    /// Convert raw angular rate to degrees per second.
    pub fn gyro_dps(raw: Raw3) -> [f32; 3] {
        [raw.x as f32 / GYRO_LSB_PER_DPS, raw.y as f32 / GYRO_LSB_PER_DPS, raw.z as f32 / GYRO_LSB_PER_DPS]
    }
}

impl<I: I2c> kumiki_class::Imu for Mpu6886<I> {
    type Error = Error<I::Error>;
    fn accel(&mut self) -> Result<kumiki_class::Vec3, Self::Error> {
        let [x, y, z] = Self::accel_g(self.accel_raw()?);
        Ok(kumiki_class::Vec3 { x, y, z })
    }
    fn gyro(&mut self) -> Result<kumiki_class::Vec3, Self::Error> {
        let [x, y, z] = Self::gyro_dps(self.gyro_raw()?);
        Ok(kumiki_class::Vec3 { x, y, z })
    }
}

impl<I: I2c> Mpu6886<I> {
    fn read(&mut self, reg: u8, buf: &mut [u8]) -> Result<(), Error<I::Error>> {
        self.i2c.write_read(self.addr, &[reg], buf).map_err(Error::I2c)
    }

    /// Read WHO_AM_I.
    pub fn who_am_i(&mut self) -> Result<u8, Error<I::Error>> {
        let mut b = [0];
        self.read(REG_WHO_AM_I, &mut b)?;
        Ok(b[0])
    }

    /// Verify the chip id and configure it.
    pub fn init(&mut self, delay: &mut impl DelayNs) -> Result<(), Error<I::Error>> {
        let id = self.who_am_i()?;
        if id != WHO_AM_I_VALUE {
            return Err(Error::WrongId(id));
        }
        for (reg, val, ms) in INIT_SEQUENCE {
            self.i2c.write(self.addr, &[reg, val]).map_err(Error::I2c)?;
            delay.delay_ms(ms);
        }
        Ok(())
    }

    /// Sleep mode (`PWR_MGMT_1.SLEEP`, keeping the clock source of `init`);
    /// the configuration is kept. [`resume`](Self::resume) undoes it.
    pub fn suspend(&mut self) -> Result<(), Error<I::Error>> {
        self.i2c.write(self.addr, &[REG_PWR_MGMT_1, 0x41]).map_err(Error::I2c)
    }

    /// Leaves sleep mode; samples are valid again after the sensors' start-up
    /// time (gyroscope ~35 ms), which this waits.
    pub fn resume(&mut self, delay: &mut impl DelayNs) -> Result<(), Error<I::Error>> {
        self.i2c.write(self.addr, &[REG_PWR_MGMT_1, 0x01]).map_err(Error::I2c)?;
        delay.delay_ms(35);
        Ok(())
    }

    /// Raw accelerometer sample.
    pub fn accel_raw(&mut self) -> Result<Raw3, Error<I::Error>> {
        let mut b = [0u8; 6];
        self.read(REG_ACCEL_XOUT_H, &mut b)?;
        Ok(be3(&b))
    }

    /// Raw gyroscope sample.
    pub fn gyro_raw(&mut self) -> Result<Raw3, Error<I::Error>> {
        let mut b = [0u8; 6];
        self.read(REG_GYRO_XOUT_H, &mut b)?;
        Ok(be3(&b))
    }
}

impl<I: embedded_hal_async::i2c::I2c> Mpu6886<I> {
    /// `async` version of [`Mpu6886::init`].
    pub async fn init_async(&mut self, delay: &mut impl embedded_hal_async::delay::DelayNs) -> Result<(), Error<I::Error>> {
        let mut b = [0];
        self.i2c.write_read(self.addr, &[REG_WHO_AM_I], &mut b).await.map_err(Error::I2c)?;
        if b[0] != WHO_AM_I_VALUE {
            return Err(Error::WrongId(b[0]));
        }
        for (reg, val, ms) in INIT_SEQUENCE {
            self.i2c.write(self.addr, &[reg, val]).await.map_err(Error::I2c)?;
            delay.delay_ms(ms).await;
        }
        Ok(())
    }

    /// `async` raw accelerometer sample.
    pub async fn accel_raw_async(&mut self) -> Result<Raw3, Error<I::Error>> {
        let mut b = [0u8; 6];
        self.i2c.write_read(self.addr, &[REG_ACCEL_XOUT_H], &mut b).await.map_err(Error::I2c)?;
        Ok(be3(&b))
    }

    /// `async` raw gyroscope sample.
    pub async fn gyro_raw_async(&mut self) -> Result<Raw3, Error<I::Error>> {
        let mut b = [0u8; 6];
        self.i2c.write_read(self.addr, &[REG_GYRO_XOUT_H], &mut b).await.map_err(Error::I2c)?;
        Ok(be3(&b))
    }
}
