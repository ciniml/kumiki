//! # kumiki-bmi270
//!
//! Driver for the Bosch Sensortec BMI270 6-axis IMU (M5Stack CoreS3, StopWatch,
//! Tab5, ...), on `embedded-hal` 1.0 I²C (blocking) and `embedded-hal-async`
//! 1.0 I²C (`async`).
//!
//! The BMI270 needs an 8 KiB configuration blob uploaded at every power-up.
//! It is taken verbatim from Bosch's BMI270_SensorAPI v2.86.1
//! (`vendor/bmi270.c`, BSD-3-Clause) and extracted at compile time.
//!
//! Configuration: accelerometer ±8 g @ 100 Hz, gyroscope ±2000 dps @ 200 Hz.
#![cfg_attr(not(test), no_std)]
#![warn(missing_docs)]

use embedded_hal::{delay::DelayNs, i2c::I2c};

/// Default 7-bit I²C address (SDO low). 0x69 with SDO high (CoreS3).
pub const DEFAULT_ADDRESS: u8 = 0x68;
/// Alternate 7-bit I²C address (SDO high).
pub const ALT_ADDRESS: u8 = 0x69;
/// CHIP_ID of the BMI270.
pub const CHIP_ID: u8 = 0x24;

const REG_CHIP_ID: u8 = 0x00;
const REG_ACC_X_LSB: u8 = 0x0C;
const REG_GYR_X_LSB: u8 = 0x12;
const REG_INTERNAL_STATUS: u8 = 0x21;
const REG_ACC_CONF: u8 = 0x40;
const REG_ACC_RANGE: u8 = 0x41;
const REG_GYR_CONF: u8 = 0x42;
const REG_GYR_RANGE: u8 = 0x43;
const REG_INIT_CTRL: u8 = 0x59;
const REG_INIT_ADDR_0: u8 = 0x5B;
const REG_INIT_DATA: u8 = 0x5E;
const REG_PWR_CONF: u8 = 0x7C;
const REG_PWR_CTRL: u8 = 0x7D;
const REG_CMD: u8 = 0x7E;
const CMD_SOFT_RESET: u8 = 0xB6;

/// LSB per g at ±8 g.
const ACCEL_LSB_PER_G: f32 = 4096.0;
/// LSB per degree/s at ±2000 dps.
const GYRO_LSB_PER_DPS: f32 = 16.384;

/// Bytes written per INIT_DATA burst.
const UPLOAD_CHUNK: usize = 32;

/// The 8 KiB configuration blob (Bosch Sensortec, BSD-3-Clause).
pub static CONFIG_FILE: [u8; 8192] = parse_config(include_str!("../vendor/bmi270.c").as_bytes());

/// Extract `bmi270_config_file[] = { 0x.., ... }` from the vendor C source at compile time.
const fn parse_config(src: &[u8]) -> [u8; 8192] {
    const KEY: &[u8] = b"bmi270_config_file[] = {";
    let mut i = 0;
    'find: loop {
        assert!(i + KEY.len() <= src.len(), "config array not found in vendor/bmi270.c");
        let mut k = 0;
        while k < KEY.len() {
            if src[i + k] != KEY[k] {
                i += 1;
                continue 'find;
            }
            k += 1;
        }
        i += KEY.len();
        break;
    }
    const fn hex(c: u8) -> u8 {
        match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => panic!("bad hex digit"),
        }
    }
    let mut out = [0u8; 8192];
    let mut n = 0;
    while src[i] != b'}' {
        if src[i] == b'0' && (src[i + 1] == b'x' || src[i + 1] == b'X') {
            assert!(n < 8192, "config array longer than 8192 bytes");
            out[n] = hex(src[i + 2]) << 4 | hex(src[i + 3]);
            n += 1;
            i += 4;
        } else {
            i += 1;
        }
    }
    assert!(n == 8192, "config array shorter than 8192 bytes");
    out
}

/// Driver error.
#[derive(Debug)]
pub enum Error<E> {
    /// I²C error.
    I2c(E),
    /// CHIP_ID did not match.
    WrongId(u8),
    /// The configuration did not take effect (INTERNAL_STATUS stayed 0).
    InitFailed(u8),
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

fn le3(b: &[u8; 6]) -> Raw3 {
    Raw3 {
        x: i16::from_le_bytes([b[0], b[1]]),
        y: i16::from_le_bytes([b[2], b[3]]),
        z: i16::from_le_bytes([b[4], b[5]]),
    }
}

/// Post-upload configuration writes.
const SENSOR_CONFIG: [(u8, u8); 5] = [
    (REG_ACC_CONF, 0xA8),  // ODR 100 Hz, normal mode, performance filter
    (REG_ACC_RANGE, 0x02), // ±8 g
    (REG_GYR_CONF, 0xA9),  // ODR 200 Hz, normal mode
    (REG_GYR_RANGE, 0x00), // ±2000 dps
    (REG_PWR_CTRL, 0x0E),  // temperature + accelerometer + gyroscope on
];

/// BMI270 driver.
pub struct Bmi270<I> {
    i2c: I,
    addr: u8,
}

impl<I> Bmi270<I> {
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

/// INIT_ADDR_0/1 payload for a byte offset into the config file (word address).
fn init_addr(offset: usize) -> [u8; 3] {
    [REG_INIT_ADDR_0, ((offset >> 1) & 0x0F) as u8, (offset >> 5) as u8]
}

impl<I: I2c> Bmi270<I> {
    fn read(&mut self, reg: u8, buf: &mut [u8]) -> Result<(), Error<I::Error>> {
        self.i2c.write_read(self.addr, &[reg], buf).map_err(Error::I2c)
    }
    fn write(&mut self, reg: u8, val: u8) -> Result<(), Error<I::Error>> {
        self.i2c.write(self.addr, &[reg, val]).map_err(Error::I2c)
    }

    /// Read CHIP_ID.
    pub fn chip_id(&mut self) -> Result<u8, Error<I::Error>> {
        let mut b = [0];
        self.read(REG_CHIP_ID, &mut b)?;
        Ok(b[0])
    }

    /// Verify the chip, upload the configuration blob and enable the sensors.
    pub fn init(&mut self, delay: &mut impl DelayNs) -> Result<(), Error<I::Error>> {
        let id = self.chip_id()?;
        if id != CHIP_ID {
            return Err(Error::WrongId(id));
        }
        self.write(REG_CMD, CMD_SOFT_RESET)?;
        delay.delay_ms(2);
        let _ = self.chip_id(); // first access after reset selects I²C mode
        self.write(REG_PWR_CONF, 0x00)?; // advanced power save off
        delay.delay_us(450);
        self.write(REG_INIT_CTRL, 0x00)?;
        let mut buf = [0u8; 1 + UPLOAD_CHUNK];
        buf[0] = REG_INIT_DATA;
        for (n, chunk) in CONFIG_FILE.chunks(UPLOAD_CHUNK).enumerate() {
            self.i2c.write(self.addr, &init_addr(n * UPLOAD_CHUNK)).map_err(Error::I2c)?;
            buf[1..1 + chunk.len()].copy_from_slice(chunk);
            self.i2c.write(self.addr, &buf[..1 + chunk.len()]).map_err(Error::I2c)?;
        }
        self.write(REG_INIT_CTRL, 0x01)?;
        let mut status = [0];
        for _ in 0..20 {
            delay.delay_ms(5);
            self.read(REG_INTERNAL_STATUS, &mut status)?;
            if status[0] & 0x0F == 0x01 {
                break;
            }
        }
        if status[0] & 0x0F != 0x01 {
            return Err(Error::InitFailed(status[0]));
        }
        for (reg, val) in SENSOR_CONFIG {
            self.write(reg, val)?;
        }
        delay.delay_ms(2);
        Ok(())
    }

    /// Raw accelerometer sample.
    pub fn accel_raw(&mut self) -> Result<Raw3, Error<I::Error>> {
        let mut b = [0u8; 6];
        self.read(REG_ACC_X_LSB, &mut b)?;
        Ok(le3(&b))
    }

    /// Raw gyroscope sample.
    pub fn gyro_raw(&mut self) -> Result<Raw3, Error<I::Error>> {
        let mut b = [0u8; 6];
        self.read(REG_GYR_X_LSB, &mut b)?;
        Ok(le3(&b))
    }
}

impl<I: embedded_hal_async::i2c::I2c> Bmi270<I> {
    async fn write_async(&mut self, reg: u8, val: u8) -> Result<(), Error<I::Error>> {
        self.i2c.write(self.addr, &[reg, val]).await.map_err(Error::I2c)
    }

    /// `async` version of [`Bmi270::init`].
    pub async fn init_async(&mut self, delay: &mut impl embedded_hal_async::delay::DelayNs) -> Result<(), Error<I::Error>> {
        let mut b = [0];
        self.i2c.write_read(self.addr, &[REG_CHIP_ID], &mut b).await.map_err(Error::I2c)?;
        if b[0] != CHIP_ID {
            return Err(Error::WrongId(b[0]));
        }
        self.write_async(REG_CMD, CMD_SOFT_RESET).await?;
        delay.delay_ms(2).await;
        let _ = self.i2c.write_read(self.addr, &[REG_CHIP_ID], &mut b).await;
        self.write_async(REG_PWR_CONF, 0x00).await?;
        delay.delay_us(450).await;
        self.write_async(REG_INIT_CTRL, 0x00).await?;
        let mut buf = [0u8; 1 + UPLOAD_CHUNK];
        buf[0] = REG_INIT_DATA;
        for (n, chunk) in CONFIG_FILE.chunks(UPLOAD_CHUNK).enumerate() {
            self.i2c.write(self.addr, &init_addr(n * UPLOAD_CHUNK)).await.map_err(Error::I2c)?;
            buf[1..1 + chunk.len()].copy_from_slice(chunk);
            self.i2c.write(self.addr, &buf[..1 + chunk.len()]).await.map_err(Error::I2c)?;
        }
        self.write_async(REG_INIT_CTRL, 0x01).await?;
        let mut status = [0];
        for _ in 0..20 {
            delay.delay_ms(5).await;
            self.i2c.write_read(self.addr, &[REG_INTERNAL_STATUS], &mut status).await.map_err(Error::I2c)?;
            if status[0] & 0x0F == 0x01 {
                break;
            }
        }
        if status[0] & 0x0F != 0x01 {
            return Err(Error::InitFailed(status[0]));
        }
        for (reg, val) in SENSOR_CONFIG {
            self.write_async(reg, val).await?;
        }
        delay.delay_ms(2).await;
        Ok(())
    }

    /// `async` raw accelerometer sample.
    pub async fn accel_raw_async(&mut self) -> Result<Raw3, Error<I::Error>> {
        let mut b = [0u8; 6];
        self.i2c.write_read(self.addr, &[REG_ACC_X_LSB], &mut b).await.map_err(Error::I2c)?;
        Ok(le3(&b))
    }

    /// `async` raw gyroscope sample.
    pub async fn gyro_raw_async(&mut self) -> Result<Raw3, Error<I::Error>> {
        let mut b = [0u8; 6];
        self.i2c.write_read(self.addr, &[REG_GYR_X_LSB], &mut b).await.map_err(Error::I2c)?;
        Ok(le3(&b))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_blob_is_extracted() {
        // First and last bytes of Bosch's bmi270_config_file (v2.86.1).
        assert_eq!(&CONFIG_FILE[..4], &[0xc8, 0x2e, 0x00, 0x2e]);
        assert_eq!(CONFIG_FILE.len(), 8192);
    }

    #[test]
    fn init_addr_is_word_address() {
        assert_eq!(init_addr(0), [0x5B, 0, 0]);
        assert_eq!(init_addr(32), [0x5B, 0, 1]); // word 16 → low nibble 0, high 1
        assert_eq!(init_addr(8160), [0x5B, 0, 255]);
        assert_eq!(init_addr(34), [0x5B, 1, 1]);
    }
}
