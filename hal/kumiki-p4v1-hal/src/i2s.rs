//! I2S0 as a full-duplex, continuously streaming master: standard Philips
//! format, 16-bit stereo, MCLK = 256 * fs from the 40 MHz crystal, TX and
//! RX on circular AHB-GDMA descriptor rings ([`ahb_dma`]) in L2MEM.
//!
//! Registers from ESP-IDF 6.0 `soc/esp32p4/register/hw_ver1/soc/i2s_reg.h`
//! and `hp_sys_clkrst_reg.h` (the I2S block is the same on v1 and v3); the
//! sequence after `hal/esp32p4/include/hal/i2s_ll.h`, `hal/i2s_hal.c` and
//! `esp_driver_i2s/i2s_std.c`. Rev v1 has no PLL_F160M for I2S (ESP-IDF
//! falls back to XTAL below rev v3), so the clock source is XTAL.
//!
//! # Full duplex
//! TX is the master and drives MCLK, BCK and WS. RX runs in slave mode on
//! the same BCK / WS inside the controller (`SIG_LOOPBACK`, what ESP-IDF's
//! full-duplex mode does), so both directions run at exactly the same rate
//! and only DIN is routed in.
//!
//! # Streaming model
//! Polled by default. [`I2s::start`] starts both directions;
//! [`I2s::poll`] does the bookkeeping and is also called by the data
//! functions, so a program that pushes and pops regularly need not call it
//! itself. An OS can instead route the DMA interrupts
//! ([`I2s::interrupt_sources`], [`I2s::set_interrupts`]) and call
//! [`I2s::on_interrupt`] from the handler. The AHB-GDMA channel is the
//! caller's choice ([`Config::dma_channel`]).
//!
//! - TX: a ring of [`TX_DESCS`] descriptors of [`DESC_LEN`] bytes the DMA
//!   loops over forever. Every descriptor carries `suc_eof`, so the DMA's
//!   EOF address register tells how far it has read. `poll` clears each
//!   descriptor the DMA has finished to **silence** and hands it to the
//!   program ([`I2s::push`]). If the program stops pushing, TX keeps
//!   running and plays silence — provided `poll` runs at least once per
//!   ring period ([`I2s::tx_ring_us`], about 85 ms at 48 kHz); otherwise
//!   the stale ring contents repeat. Latency is at most one ring.
//! - RX: a ring of [`RX_DESCS`] descriptors. The DMA writes the received
//!   length into each descriptor and clears its owner bit when it closes
//!   it; [`I2s::pop`] copies from closed descriptors in order (any length,
//!   so no byte is lost when a descriptor is closed early) and hands each
//!   one back once it is empty. The program must read at least once per
//!   ring period, or the DMA overwrites unread data
//!   ([`Stats::rx_overruns`]).
//!
//! The CPU touches the rings through the non-cacheable alias of L2MEM, so
//! no cache maintenance is needed.
//!
//! Byte layout: frames of 4 bytes, left sample first, each sample a
//! little-endian `i16` (`[[i16; 2]]` in memory order).
//!
//! [`ahb_dma`]: crate::ahb_dma

use core::ptr::addr_of;
use core::sync::atomic::{Ordering, fence};

use crate::ahb_dma::{self, Descriptor, RxChannel, TxChannel};
use crate::gpio::{AnyPin, Pull};
use crate::{cache, clear_bits, log, rd, set_bits, set_bits_field, time, wait_us, wr};

// Clocks and resets (hp_sys_clkrst_reg.h).
const CLKRST: usize = 0x500E_6000;
const SOC_CLK_CTRL2: usize = CLKRST + 0x1C;
const I2S0_APB_CLK_EN: u32 = 1 << 14;
const PERI_CLK_CTRL11: usize = CLKRST + 0x44;
const PERI_CLK_CTRL12: usize = CLKRST + 0x48;
const PERI_CLK_CTRL13: usize = CLKRST + 0x4C;
const PERI_CLK_CTRL14: usize = CLKRST + 0x50;
const HP_RST_EN2: usize = CLKRST + 0xC8;
const RST_EN_I2S0_APB: u32 = 1 << 4;
/// LP_CLKRST_HP_CLK_CTRL.HP_PAD_I2S0_MCLK_EN (lp_clkrst_reg.h).
const LP_HP_CLK_CTRL: usize = 0x5011_1040;
const HP_PAD_I2S0_MCLK_EN: u32 = 1 << 12;

// I2S0 (DR_REG_I2S0_BASE; i2s_reg.h).
const I2S0: usize = 0x500C_6000;
const INT_ENA: usize = I2S0 + 0x14;
const INT_CLR: usize = I2S0 + 0x18;
const RX_CONF: usize = I2S0 + 0x20;
const TX_CONF: usize = I2S0 + 0x24;
const RX_CONF1: usize = I2S0 + 0x28;
const TX_CONF1: usize = I2S0 + 0x2C;
const RX_TDM_CTRL: usize = I2S0 + 0x50;
const TX_TDM_CTRL: usize = I2S0 + 0x54;
const RXEOF_NUM: usize = I2S0 + 0x64;

// RX_CONF / TX_CONF bits.
const C_RESET: u32 = 1 << 0;
const C_FIFO_RESET: u32 = 1 << 1;
const C_START: u32 = 1 << 2;
const C_SLAVE_MOD: u32 = 1 << 3;
const C_UPDATE: u32 = 1 << 8;
const C_MONO_FST_VLD: u32 = 1 << 9;
const C_PCM_BYPASS: u32 = 1 << 12;
const C_MSB_SHIFT: u32 = 1 << 13;
const TX_BCK_NO_DLY: u32 = 1 << 14;
const C_LEFT_ALIGN: u32 = 1 << 15;
const C_TDM_EN: u32 = 1 << 19;
const C_BCK_DIV_S: u32 = 21;
const RX_PCM_CONF_DEFAULT: u32 = 1 << 10;
const TX_SIG_LOOPBACK: u32 = 1 << 30;

// GPIO matrix signals (gpio_sig_map.h).
const SIG_BCK_OUT: u16 = 25;
const SIG_MCLK_OUT: u16 = 26;
const SIG_WS_OUT: u16 = 27;
const SIG_SD_OUT: u16 = 28;
const SIG_SD_IN: u16 = 28;

/// The I2S source clock (XTAL).
pub const SOURCE_HZ: u32 = 40_000_000;
/// MCLK / fs.
pub const MCLK_MULTIPLE: u32 = 256;
/// BCK / fs: two 16-bit slots.
const BCK_PER_FRAME: u32 = 32;
/// MCLK / BCK.
const BCK_DIV: u32 = MCLK_MULTIPLE / BCK_PER_FRAME;
/// Bytes per frame (two `i16`).
pub const FRAME_BYTES: usize = 4;

/// Bytes per DMA descriptor (128 frames, 2.67 ms at 48 kHz).
pub const DESC_LEN: usize = 512;
/// TX descriptors in the ring.
pub const TX_DESCS: usize = 32;
/// RX descriptors in the ring.
pub const RX_DESCS: usize = 32;
const TX_RING: usize = DESC_LEN * TX_DESCS;
const RX_RING: usize = DESC_LEN * RX_DESCS;

/// Descriptors and buffers (in .bss, L2MEM).
#[repr(C, align(64))]
struct DmaMem {
    tx_desc: [Descriptor; TX_DESCS],
    rx_desc: [Descriptor; RX_DESCS],
    tx_buf: [u8; TX_RING],
    rx_buf: [u8; RX_RING],
}

const ZERO_DESC: Descriptor = Descriptor { dw0: 0, buf: 0, next: 0 };
static mut DMA_MEM: DmaMem = DmaMem {
    tx_desc: [ZERO_DESC; TX_DESCS],
    rx_desc: [ZERO_DESC; RX_DESCS],
    tx_buf: [0; TX_RING],
    rx_buf: [0; RX_RING],
};

fn tx_desc_addr(i: usize) -> usize {
    unsafe { addr_of!(DMA_MEM.tx_desc[i]) as usize }
}
fn rx_desc_addr(i: usize) -> usize {
    unsafe { addr_of!(DMA_MEM.rx_desc[i]) as usize }
}
fn tx_buf_addr() -> usize {
    unsafe { addr_of!(DMA_MEM.tx_buf) as usize }
}
fn rx_buf_addr() -> usize {
    unsafe { addr_of!(DMA_MEM.rx_buf) as usize }
}
/// `dw0` of descriptor at `addr`, through the uncached alias.
fn desc_dw0(addr: usize) -> u32 {
    unsafe { core::ptr::read_volatile(ahb_dma::uncached(addr) as *const u32) }
}
fn set_desc(addr: usize, d: Descriptor) {
    let p = ahb_dma::uncached(addr) as *mut u32;
    unsafe {
        core::ptr::write_volatile(p, d.dw0);
        core::ptr::write_volatile(p.add(1), d.buf);
        core::ptr::write_volatile(p.add(2), d.next);
    }
}
fn set_dw0(addr: usize, dw0: u32) {
    unsafe { core::ptr::write_volatile(ahb_dma::uncached(addr) as *mut u32, dw0) };
}

/// The I2S0 controller (token).
pub struct I2s0(pub(crate) ());

impl I2s0 {
    /// The controller token without taking [`Peripherals`](crate::Peripherals).
    ///
    /// # Safety
    /// Nothing else may use I2S0 (or AHB-GDMA channel 0) meanwhile.
    pub unsafe fn steal() -> Self {
        I2s0(())
    }
}

/// The pins of the I2S bus.
pub struct Pins {
    /// MCLK output.
    pub mclk: AnyPin,
    /// BCK output.
    pub bclk: AnyPin,
    /// WS (LRCK) output.
    pub ws: AnyPin,
    /// Serial data output (to the DAC).
    pub dout: AnyPin,
    /// Serial data input (from the ADC).
    pub din: AnyPin,
}

/// Configuration.
#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Sample rate in Hz (about 611 Hz to 78 kHz: the MCLK divider from
    /// 40 MHz must be at least 2).
    pub sample_rate: u32,
    /// AHB-GDMA channel (0..[`ahb_dma::CHANNELS`]) for both directions; the
    /// caller picks one no other driver uses.
    pub dma_channel: usize,
}

impl Default for Config {
    fn default() -> Self {
        Config { sample_rate: 48_000, dma_channel: 0 }
    }
}

/// Configuration error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The sample rate cannot be made from 40 MHz.
    SampleRate,
    /// No such AHB-GDMA channel.
    DmaChannel,
}

/// Interrupt-matrix sources of the I2S DMA (rev v1 numbering).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterruptSources {
    /// TX direction: raised per TX descriptor sent ([`ahb_dma::TX_INT_EOF`]).
    pub tx: u32,
    /// RX direction: raised per RX descriptor closed ([`ahb_dma::RX_INT_SUC_EOF`]).
    pub rx: u32,
}

/// The MCLK divider: `SOURCE_HZ / (integer + numerator / denominator)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MclkDivider {
    /// Integer part (2..=255).
    pub integer: u32,
    /// Fraction numerator (0 if none).
    pub numerator: u32,
    /// Fraction denominator (0 if none, else up to 511).
    pub denominator: u32,
}

impl MclkDivider {
    /// The best divider for `mclk_hz` from [`SOURCE_HZ`].
    pub fn for_mclk(mclk_hz: u32) -> Option<Self> {
        let integer = SOURCE_HZ / mclk_hz;
        if !(2..256).contains(&integer) {
            return None;
        }
        let rem = (SOURCE_HZ % mclk_hz) as u64;
        if rem == 0 {
            return Some(MclkDivider { integer, numerator: 0, denominator: 0 });
        }
        let m = mclk_hz as u64;
        // Search the denominator; reject coefficient sets with div_y == 0,
        // on which the hardware ignores the fraction (M5Unified
        // Speaker_Class.cpp notes the same).
        let mut best: Option<(u64, u32, u32)> = None;
        for a in 2u32..512 {
            let b = ((rem * a as u64 + m / 2) / m) as u32;
            if b == 0 || b >= a {
                continue;
            }
            let (_, y, _, _) = coefficients(b, a);
            if y == 0 {
                continue;
            }
            // |rem/m - b/a| scaled by m*a.
            let err = (rem * a as u64).abs_diff(b as u64 * m);
            let err = err * 512 / a as u64;
            if best.is_none_or(|(e, _, _)| err < e) {
                best = Some((err, b, a));
            }
        }
        best.map(|(_, b, a)| MclkDivider { integer, numerator: b, denominator: a })
    }

    /// The resulting MCLK in millihertz.
    pub fn mclk_mhz(&self) -> u64 {
        let (n, b, a) = (self.integer as u64, self.numerator as u64, self.denominator.max(1) as u64);
        SOURCE_HZ as u64 * 1000 * a / (n * a + b)
    }
}

/// (x, y, z, yn1) from the fraction b/a (`_i2s_ll_tx_set_mclk`).
fn coefficients(b: u32, a: u32) -> (u32, u32, u32, u32) {
    if a == 0 || b == 0 {
        return (0, 0, 0, 0);
    }
    let yn1 = (b * 2 > a) as u32;
    let z = if yn1 != 0 { a - b } else { b };
    (a / z - 1, a % z, z, yn1)
}

/// What [`I2s::poll`] reports.
#[derive(Clone, Copy, Debug, Default)]
pub struct Status {
    /// Bytes [`I2s::push`] can take now.
    pub tx_space: usize,
    /// Bytes [`I2s::pop`] can return now.
    pub rx_available: usize,
}

/// Counters.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    /// Bytes pushed by the program.
    pub tx_bytes: u64,
    /// Times TX ran dry after the program had pushed data (silence was
    /// played until the next push).
    pub tx_underruns: u32,
    /// Bytes popped by the program.
    pub rx_bytes: u64,
    /// Times `poll` found the whole RX ring full: the DMA was overwriting
    /// unread data.
    pub rx_overruns: u32,
    /// RX descriptors that were closed with fewer than [`DESC_LEN`] bytes.
    pub rx_short_descs: u32,
    /// The last length of such a descriptor.
    pub rx_last_short_len: usize,
    /// TX descriptors the DMA has finished.
    pub tx_descs_done: u64,
    /// Raw TX DMA interrupt bits (`OUT_INT_RAW`).
    pub tx_dma_int_raw: u32,
    /// Raw RX DMA interrupt bits (`IN_INT_RAW`).
    pub rx_dma_int_raw: u32,
}

/// I2S0, full duplex, streaming (see the module documentation).
pub struct I2s {
    _pins: Pins,
    tx: TxChannel,
    rx: RxChannel,
    config: Config,
    divider: MclkDivider,
    running: bool,
    /// TX descriptors the DMA has finished (absolute).
    tx_done: u64,
    /// Absolute TX byte position the program writes next.
    tx_w: u64,
    /// Last seen `OUT_EOF_DES_ADDR`.
    tx_last_eof: usize,
    /// No pushed data is pending (an underrun is counted on the transition).
    tx_dry: bool,
    /// Next RX descriptor to read, and the bytes already read from it.
    rx_head: usize,
    rx_off: usize,
    stats: Stats,
}

impl I2s {
    /// Sets up I2S0 and its DMA rings with `pins` at `config`; the bus
    /// stays idle until [`start`](Self::start).
    pub fn new(_inst: I2s0, pins: Pins, config: Config) -> Result<Self, Error> {
        if config.dma_channel >= ahb_dma::CHANNELS {
            return Err(Error::DmaChannel);
        }
        let mclk = config.sample_rate.checked_mul(MCLK_MULTIPLE).ok_or(Error::SampleRate)?;
        let divider = MclkDivider::for_mclk(mclk).ok_or(Error::SampleRate)?;

        // Bus clock, reset (i2s_ll_enable_bus_clock, i2s_ll_reset_register).
        set_bits(SOC_CLK_CTRL2, I2S0_APB_CLK_EN);
        set_bits(LP_HP_CLK_CTRL, HP_PAD_I2S0_MCLK_EN);
        set_bits(HP_RST_EN2, RST_EN_I2S0_APB);
        clear_bits(HP_RST_EN2, RST_EN_I2S0_APB);

        // Module clocks from XTAL, both dividers the same, MCLK from TX.
        set_bits_field(PERI_CLK_CTRL13, 11, 2, 0); // tx src = XTAL
        set_bits(PERI_CLK_CTRL13, 1 << 10); // tx clk en
        set_bits_field(PERI_CLK_CTRL11, 25, 2, 0); // rx src = XTAL
        set_bits(PERI_CLK_CTRL11, 1 << 24); // rx clk en
        set_tx_divider(&divider);
        set_rx_divider(&divider);
        set_bits(PERI_CLK_CTRL14, 1 << 19); // mst_clk_sel: MCLK from TX

        // Format: Philips, 16-bit slots, 2 slots, WS 16 BCK wide.
        let conf1 = 15 | (15 << 14) | (15 << 19) | (15 << 27);
        let common = C_MONO_FST_VLD | C_PCM_BYPASS | C_MSB_SHIFT | C_LEFT_ALIGN | C_TDM_EN | ((BCK_DIV - 1) << C_BCK_DIV_S);
        unsafe {
            wr(INT_ENA, 0);
            wr(INT_CLR, 0xF);
            // TX master; BCK keeps running when the FIFO is empty
            // (TX_STOP_EN = 0); RX takes BCK / WS from TX.
            wr(TX_CONF, common | TX_BCK_NO_DLY | TX_SIG_LOOPBACK);
            wr(RX_CONF, common | RX_PCM_CONF_DEFAULT | C_SLAVE_MOD);
            wr(TX_CONF1, conf1);
            wr(RX_CONF1, conf1);
            wr(TX_TDM_CTRL, (1 << 16) | 0x3);
            wr(RX_TDM_CTRL, (1 << 16) | 0x3);
            // EOF makes the DMA close the RX descriptor. i2s_reg.h says
            // every (bits_mod + 1) * (n + 1) bits (= 512 bytes here), but on
            // rev v1 with 16-bit slots n = 255 closed every descriptor at
            // 256 bytes (measured on a Tab5). That is a divisor of
            // DESC_LEN and a multiple of the frame size, so `pop` sees a
            // steady stream of 256-byte descriptors; n = DESC_LEN - 1 would
            // likely fill them but is untested.
            wr(RXEOF_NUM, (DESC_LEN / 2 - 1) as u32);
        }

        // Pins.
        for (pin, sig) in [(&pins.mclk, SIG_MCLK_OUT), (&pins.bclk, SIG_BCK_OUT), (&pins.ws, SIG_WS_OUT), (&pins.dout, SIG_SD_OUT)] {
            pin.set_level(false);
            pin.configure(Pull::None, false);
            pin.connect_output_gpio_oe(sig);
            pin.set_output_enable(true);
        }
        pins.din.set_output_enable(false);
        pins.din.configure(Pull::None, false);
        pins.din.connect_input(SIG_SD_IN);

        // DMA.
        ahb_dma::init();
        // SAFETY: the caller picked `dma_channel` for this driver alone.
        let (tx, rx) = unsafe { (TxChannel::steal(config.dma_channel), RxChannel::steal(config.dma_channel)) };
        let (Some(tx), Some(rx)) = (tx, rx) else { return Err(Error::DmaChannel) };
        // Lines of the rings still in L1 D (from .bss being cleared) must
        // not be written back over them later.
        let (m, n) = (addr_of!(DMA_MEM) as usize, core::mem::size_of::<DmaMem>());
        cache::writeback(m, n, false);
        cache::invalidate(m, n, false);

        log!(
            "fs {} Hz: mclk div {} {}/{} -> mclk {} mHz, bck div {}",
            config.sample_rate,
            divider.integer,
            divider.numerator,
            divider.denominator,
            divider.mclk_mhz(),
            BCK_DIV
        );
        Ok(I2s {
            _pins: pins,
            tx,
            rx,
            config,
            divider,
            running: false,
            tx_done: 0,
            tx_w: 0,
            tx_last_eof: 0,
            tx_dry: true,
            rx_head: 0,
            rx_off: 0,
            stats: Stats::default(),
        })
    }

    /// The configuration.
    pub fn config(&self) -> Config {
        self.config
    }

    /// The MCLK divider in use.
    pub fn divider(&self) -> MclkDivider {
        self.divider
    }

    /// The sample rate the divider actually gives, in millihertz.
    pub fn actual_sample_rate_mhz(&self) -> u64 {
        self.divider.mclk_mhz() / MCLK_MULTIPLE as u64
    }

    /// One TX ring period in microseconds: how often [`poll`](Self::poll)
    /// must run at least so that a starved TX plays silence.
    pub fn tx_ring_us(&self) -> u64 {
        (TX_RING / FRAME_BYTES) as u64 * 1_000_000 / self.config.sample_rate as u64
    }

    /// Whether [`start`](Self::start) has run (and [`stop`](Self::stop)
    /// not since).
    pub fn is_running(&self) -> bool {
        self.running
    }

    /// (Re)builds both rings (TX all silence, nothing received) and starts
    /// both directions: the DMA first, then TX (MCLK / BCK / WS start), then
    /// RX. Data pushed before `start` is discarded.
    pub fn start(&mut self) {
        if self.running {
            self.stop();
        }
        self.build_rings();
        self.tx.configure(ahb_dma::peri::I2S0, false);
        self.rx.configure(ahb_dma::peri::I2S0);
        self.tx_last_eof = self.tx.eof_desc_addr();
        self.tx_done = 0;
        // Descriptor 0 is the first in flight; the program may fill the rest.
        self.tx_w = DESC_LEN as u64;
        self.tx_dry = true;
        self.rx_head = 0;
        self.rx_off = 0;

        // i2s_ll_{tx,rx}_reset, _reset_fifo (write-to-trigger bits).
        set_bits(TX_CONF, C_RESET | C_FIFO_RESET);
        set_bits(RX_CONF, C_RESET | C_FIFO_RESET);
        unsafe { wr(INT_CLR, 0xF) };

        fence(Ordering::SeqCst);
        self.tx.start(tx_desc_addr(0));
        // Let the DMA fill the I2S TX FIFO before the bus starts.
        time::delay_us(50);
        update(TX_CONF);
        set_bits(TX_CONF, C_START);
        self.rx.start(rx_desc_addr(0));
        update(RX_CONF);
        set_bits(RX_CONF, C_START);
        self.running = true;
    }

    /// Stops both directions (the bus clocks stop).
    pub fn stop(&mut self) {
        clear_bits(RX_CONF, C_START);
        clear_bits(TX_CONF, C_START);
        self.rx.stop();
        self.tx.stop();
        self.running = false;
    }

    fn build_rings(&mut self) {
        let tx_buf = tx_buf_addr();
        let rx_buf = rx_buf_addr();
        unsafe {
            core::ptr::write_bytes(ahb_dma::uncached(tx_buf) as *mut u8, 0, TX_RING);
        }
        for i in 0..TX_DESCS {
            set_desc(
                tx_desc_addr(i),
                Descriptor {
                    dw0: Descriptor::dw0(DESC_LEN, DESC_LEN, true, true),
                    buf: (tx_buf + i * DESC_LEN) as u32,
                    next: tx_desc_addr((i + 1) % TX_DESCS) as u32,
                },
            );
        }
        for i in 0..RX_DESCS {
            set_desc(
                rx_desc_addr(i),
                Descriptor {
                    dw0: Descriptor::dw0(DESC_LEN, 0, false, true),
                    buf: (rx_buf + i * DESC_LEN) as u32,
                    next: rx_desc_addr((i + 1) % RX_DESCS) as u32,
                },
            );
        }
        fence(Ordering::SeqCst);
    }

    /// The interrupt-matrix sources of this stream's DMA directions, for
    /// an OS that routes them to a CPU interrupt.
    pub fn interrupt_sources(&self) -> InterruptSources {
        let n = self.tx.number();
        InterruptSources { tx: ahb_dma::tx_interrupt_source(n), rx: ahb_dma::rx_interrupt_source(n) }
    }

    /// Enables (or disables) the DMA interrupts: TX on every descriptor
    /// sent, RX on every descriptor closed (plus descriptor errors), i.e.
    /// [`DESC_LEN`] bytes apart in each direction. The handler calls
    /// [`on_interrupt`](Self::on_interrupt). Without them everything works
    /// by polling, as before.
    pub fn set_interrupts(&mut self, enable: bool) {
        let (tx, rx) = if enable {
            (ahb_dma::TX_INT_EOF | ahb_dma::TX_INT_DSCR_ERR, ahb_dma::RX_INT_SUC_EOF | ahb_dma::RX_INT_DSCR_ERR)
        } else {
            (0, 0)
        };
        self.tx.clear_int(tx | ahb_dma::TX_INT_EOF);
        self.rx.clear_int(rx | ahb_dma::RX_INT_SUC_EOF);
        self.tx.set_int_enable(tx);
        self.rx.set_int_enable(rx);
    }

    /// Interrupt handler body: clears this stream's raised DMA interrupt
    /// bits and does the [`poll`](Self::poll) bookkeeping. Returns the poll
    /// result (room for TX, data for RX), so the handler can wake the
    /// tasks waiting for either.
    pub fn on_interrupt(&mut self) -> Status {
        let (t, r) = (self.tx.int_status(), self.rx.int_status());
        self.tx.clear_int(t);
        self.rx.clear_int(r);
        self.poll()
    }

    /// Bookkeeping: hands TX descriptors the DMA has finished back to the
    /// program (cleared to silence) and checks the RX ring for an overrun.
    /// Called by every data function; call it at least once per ring
    /// period ([`tx_ring_us`](Self::tx_ring_us)) when not pushing or
    /// popping. Safe to call from an interrupt handler that owns `self`
    /// (see [`on_interrupt`](Self::on_interrupt)).
    pub fn poll(&mut self) -> Status {
        if !self.running {
            return Status::default();
        }
        // TX: the DMA has finished every descriptor up to the EOF address.
        let eof = self.tx.eof_desc_addr();
        if eof != self.tx_last_eof {
            self.tx_last_eof = eof;
            let base = tx_desc_addr(0);
            if eof >= base && eof < base + TX_DESCS * 12 && (eof - base).is_multiple_of(12) {
                let idx = (eof - base) / 12;
                let cur = (self.tx_done % TX_DESCS as u64) as usize;
                let n = (idx + TX_DESCS - cur) % TX_DESCS + 1;
                let buf = ahb_dma::uncached(tx_buf_addr());
                for k in 0..n {
                    let d = (cur + k) % TX_DESCS;
                    unsafe { core::ptr::write_bytes((buf + d * DESC_LEN) as *mut u8, 0, DESC_LEN) };
                }
                fence(Ordering::SeqCst);
                self.tx_done += n as u64;
                self.stats.tx_descs_done = self.tx_done;
            }
        }
        // The descriptor after the last finished one is in flight.
        let min_w = (self.tx_done + 1) * DESC_LEN as u64;
        if self.tx_w < min_w {
            if !self.tx_dry {
                self.stats.tx_underruns += 1;
                self.tx_dry = true;
            }
            self.tx_w = min_w;
        }
        // RX: all descriptors closed means the DMA is overwriting unread data.
        let (avail, full) = self.rx_scan();
        if full {
            self.stats.rx_overruns += 1;
        }
        Status { tx_space: self.tx_space_raw(), rx_available: avail }
    }

    fn tx_space_raw(&self) -> usize {
        ((self.tx_done + TX_DESCS as u64) * DESC_LEN as u64 - self.tx_w) as usize
    }

    /// Bytes [`push`](Self::push) can take now.
    pub fn tx_space(&mut self) -> usize {
        self.poll().tx_space
    }

    /// Queues `data` for playback; returns how many bytes were taken (0 when
    /// the ring is full or I2S is stopped).
    pub fn push(&mut self, data: &[u8]) -> usize {
        let space = self.poll().tx_space;
        let n = data.len().min(space);
        if n == 0 {
            return 0;
        }
        let pos = (self.tx_w % TX_RING as u64) as usize;
        let first = n.min(TX_RING - pos);
        let buf = ahb_dma::uncached(tx_buf_addr()) as *mut u8;
        unsafe {
            core::ptr::copy_nonoverlapping(data.as_ptr(), buf.add(pos), first);
            core::ptr::copy_nonoverlapping(data.as_ptr().add(first), buf, n - first);
        }
        fence(Ordering::SeqCst);
        self.tx_w += n as u64;
        self.tx_dry = false;
        self.stats.tx_bytes += n as u64;
        n
    }

    /// Queues whole frames; returns how many were taken.
    pub fn push_frames(&mut self, frames: &[[i16; 2]]) -> usize {
        let n = frames.len().min(self.tx_space() / FRAME_BYTES);
        let bytes = unsafe { core::slice::from_raw_parts(frames.as_ptr() as *const u8, n * FRAME_BYTES) };
        self.push(bytes) / FRAME_BYTES
    }

    /// Counts the received bytes ready from `rx_head` on, and whether every
    /// descriptor is closed.
    fn rx_scan(&mut self) -> (usize, bool) {
        let mut total = 0;
        let mut closed = 0;
        for k in 0..RX_DESCS {
            let i = (self.rx_head + k) % RX_DESCS;
            let dw0 = desc_dw0(rx_desc_addr(i));
            if dw0 & Descriptor::OWNER_DMA != 0 {
                break;
            }
            let len = Descriptor::length_of(dw0);
            total += if k == 0 { len.saturating_sub(self.rx_off) } else { len };
            closed += 1;
        }
        (total, closed == RX_DESCS)
    }

    /// Bytes [`pop`](Self::pop) can return now.
    pub fn rx_available(&mut self) -> usize {
        self.poll().rx_available
    }

    /// Copies received bytes into `out`; returns how many.
    pub fn pop(&mut self, out: &mut [u8]) -> usize {
        if !self.running {
            return 0;
        }
        let buf = ahb_dma::uncached(rx_buf_addr()) as *const u8;
        let mut n = 0;
        while n < out.len() {
            let addr = rx_desc_addr(self.rx_head);
            let dw0 = desc_dw0(addr);
            if dw0 & Descriptor::OWNER_DMA != 0 {
                break;
            }
            fence(Ordering::SeqCst);
            let len = Descriptor::length_of(dw0);
            let take = (len - self.rx_off.min(len)).min(out.len() - n);
            unsafe {
                core::ptr::copy_nonoverlapping(buf.add(self.rx_head * DESC_LEN + self.rx_off), out.as_mut_ptr().add(n), take);
            }
            n += take;
            self.rx_off += take;
            if self.rx_off >= len {
                if len != DESC_LEN {
                    self.stats.rx_short_descs += 1;
                    self.stats.rx_last_short_len = len;
                }
                fence(Ordering::SeqCst);
                set_dw0(addr, Descriptor::dw0(DESC_LEN, 0, false, true));
                self.rx_head = (self.rx_head + 1) % RX_DESCS;
                self.rx_off = 0;
            }
        }
        self.stats.rx_bytes += n as u64;
        n
    }

    /// Copies whole received frames into `out`; returns how many.
    pub fn pop_frames(&mut self, out: &mut [[i16; 2]]) -> usize {
        let n = out.len().min(self.rx_available() / FRAME_BYTES);
        let bytes = unsafe { core::slice::from_raw_parts_mut(out.as_mut_ptr() as *mut u8, n * FRAME_BYTES) };
        self.pop(bytes) / FRAME_BYTES
    }

    /// The counters, with the DMA's raw interrupt bits.
    pub fn stats(&self) -> Stats {
        Stats { tx_dma_int_raw: self.tx.int_raw(), rx_dma_int_raw: self.rx.int_raw(), ..self.stats }
    }
}

/// Sets `UPDATE` in `conf` and waits for the hardware to clear it.
fn update(conf: usize) {
    set_bits(conf, C_UPDATE);
    if !wait_us(1000, || unsafe { rd(conf) } & C_UPDATE == 0) {
        log!("update of {conf:#x} timed out");
    }
}

/// `i2s_ll_tx_set_raw_clk_div` for I2S0, with ESP-IDF's workaround: a
/// small division first, then the coefficients in this order.
fn set_tx_divider(d: &MclkDivider) {
    let (x, y, z, yn1) = coefficients(d.numerator, d.denominator);
    set_bits_field(PERI_CLK_CTRL13, 13, 8, 2);
    set_bits_field(PERI_CLK_CTRL14, 18, 1, 0);
    set_bits_field(PERI_CLK_CTRL14, 0, 9, 1);
    set_bits_field(PERI_CLK_CTRL14, 9, 9, 0);
    set_bits_field(PERI_CLK_CTRL13, 21, 9, 0);
    set_bits_field(PERI_CLK_CTRL14, 18, 1, yn1);
    set_bits_field(PERI_CLK_CTRL14, 9, 9, z);
    set_bits_field(PERI_CLK_CTRL14, 0, 9, y);
    set_bits_field(PERI_CLK_CTRL13, 21, 9, x);
    set_bits_field(PERI_CLK_CTRL13, 13, 8, d.integer);
}

/// `i2s_ll_rx_set_raw_clk_div` for I2S0.
fn set_rx_divider(d: &MclkDivider) {
    let (x, y, z, yn1) = coefficients(d.numerator, d.denominator);
    set_bits_field(PERI_CLK_CTRL12, 0, 8, 2);
    set_bits_field(PERI_CLK_CTRL13, 9, 1, 0);
    set_bits_field(PERI_CLK_CTRL12, 17, 9, 1);
    set_bits_field(PERI_CLK_CTRL13, 0, 9, 0);
    set_bits_field(PERI_CLK_CTRL12, 8, 9, 0);
    set_bits_field(PERI_CLK_CTRL13, 9, 1, yn1);
    set_bits_field(PERI_CLK_CTRL13, 0, 9, z);
    set_bits_field(PERI_CLK_CTRL12, 17, 9, y);
    set_bits_field(PERI_CLK_CTRL12, 8, 9, x);
    set_bits_field(PERI_CLK_CTRL12, 0, 8, d.integer);
}
