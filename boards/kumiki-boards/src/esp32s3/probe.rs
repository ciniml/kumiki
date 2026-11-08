//! Board-detection probes that need no peripheral driver.

use esp_hal::{
    delay::Delay,
    gpio::{DriveMode, Flex, InputConfig, Level, OutputConfig, Pin, Pull},
};

/// I²C bus recovery (NXP UM10204 §3.1.16): if a target holds SDA low (e.g.
/// the MCU was reset in the middle of a read while the device stayed powered),
/// clock SCL up to 9 times until SDA is released, then issue a STOP.
///
/// Returns `true` if SDA is high (bus free) afterwards. Only acts when SDA is
/// actually stuck, so a healthy bus sees no extra traffic.
pub fn i2c_bus_recover<'a>(scl: impl Pin + 'a, sda: impl Pin + 'a) -> bool {
    let delay = Delay::new();
    let od = OutputConfig::default().with_drive_mode(DriveMode::OpenDrain).with_pull(Pull::Up);
    let mut scl = Flex::new(scl);
    let mut sda = Flex::new(sda);
    for p in [&mut scl, &mut sda] {
        p.apply_output_config(&od);
        p.apply_input_config(&InputConfig::default().with_pull(Pull::Up));
        p.set_high();
        p.set_input_enable(true);
        p.set_output_enable(true);
    }
    delay.delay_micros(10);
    if sda.is_high() {
        return true;
    }
    for _ in 0..9 {
        scl.set_low();
        delay.delay_micros(5);
        scl.set_high();
        delay.delay_micros(5);
        if sda.is_high() {
            break;
        }
    }
    // STOP: SDA low → high while SCL is high.
    scl.set_low();
    delay.delay_micros(5);
    sda.set_low();
    delay.delay_micros(5);
    scl.set_high();
    delay.delay_micros(5);
    sda.set_high();
    delay.delay_micros(5);
    sda.is_high()
}

/// Read the 3 RDDID (0x04) bytes of a MIPI-DCS panel on a 3-wire SPI bus
/// (bidirectional SDA), bit-banged on GPIOs, after a hardware reset.
///
/// Mirrors M5GFX's `_read_panel_id` (one dummy clock after the command). For the
/// GC9107 on AtomS3 the result is `[0x00, 0x91, 0x07]`.
pub fn read_dcs_id_3wire<'a>(
    sck: impl Pin + 'a,
    sda: impl Pin + 'a,
    cs: impl Pin + 'a,
    dc: impl Pin + 'a,
    rst: impl Pin + 'a,
) -> [u8; 3] {
    let delay = Delay::new();
    let out = OutputConfig::default();
    let mut sck = Flex::new(sck);
    let mut sda = Flex::new(sda);
    let mut cs = Flex::new(cs);
    let mut dc = Flex::new(dc);
    let mut rst = Flex::new(rst);
    for p in [&mut sck, &mut sda, &mut cs, &mut dc, &mut rst] {
        p.apply_output_config(&out);
        p.set_output_enable(true);
    }
    sck.set_low();
    cs.set_high();
    rst.set_low();
    delay.delay_millis(10);
    rst.set_high();
    delay.delay_millis(20);

    let half = || delay.delay_micros(1);
    cs.set_low();
    dc.set_low();
    for i in (0..8).rev() {
        sda.set_level(Level::from((0x04u8 >> i) & 1 != 0));
        half();
        sck.set_high();
        half();
        sck.set_low();
    }
    dc.set_high();
    sda.set_output_enable(false);
    sda.apply_input_config(&InputConfig::default());
    sda.set_input_enable(true);
    // One dummy clock.
    half();
    sck.set_high();
    half();
    sck.set_low();
    let mut id = [0u8; 3];
    for byte in id.iter_mut() {
        for _ in 0..8 {
            half();
            sck.set_high();
            *byte = (*byte << 1) | sda.is_high() as u8;
            half();
            sck.set_low();
        }
    }
    cs.set_high();
    id
}
