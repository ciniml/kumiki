//! Minimal AHB GDMA ("AHB_PDMA") channel driver: just what a peripheral
//! streaming through a circular descriptor ring needs (used by [`i2s`]).
//!
//! Registers from ESP-IDF 6.0 `soc/esp32p4/register/hw_ver1/soc/
//! ahb_dma_reg.h`; the sequence after `esp_hal_dma/esp32p4/include/hal/
//! ahb_dma_ll.h` and `gdma_ll.h` (only the parts that exist on rev v1: no
//! burst-size selection, no weighted arbitration). Three TX and three RX
//! channels; each direction of a channel is independent.
//!
//! Descriptors and buffers must lie in L2MEM (the DMA's address window is
//! set to 0x4000_0000..0x4FFC_0000, as ESP-IDF does). L2MEM sits behind the
//! L1 data cache, so the CPU should access DMA memory through the
//! non-cacheable alias ([`uncached`]) or maintain the cache itself.
//!
//! [`i2s`]: crate::i2s

use crate::{clear_bits, rd, set_bits, wr};

// Clock and reset (hp_sys_clkrst_reg.h).
const SOC_CLK_CTRL1: usize = 0x500E_6018;
const AHB_PDMA_SYS_CLK_EN: u32 = 1 << 3;
const HP_RST_EN1: usize = 0x500E_60C4;
const RST_EN_AHB_PDMA: u32 = 1 << 1;

const BASE: usize = 0x5008_5000;
const MISC_CONF: usize = BASE + 0x64;
const MISC_AHBM_RST_INTER: u32 = 1 << 0;
const MISC_CLK_EN: u32 = 1 << 3;
const INTR_MEM_START: usize = BASE + 0x3C4;
const INTR_MEM_END: usize = BASE + 0x3C8;

// Per channel (stride 0xC0 for the configuration blocks, 0x10 for the
// interrupt blocks, 4 for the link address registers).
const fn in_int(n: usize) -> usize {
    BASE + 0x00 + 0x10 * n
}
const fn out_int(n: usize) -> usize {
    BASE + 0x30 + 0x10 * n
}
const fn in_ch(n: usize) -> usize {
    BASE + 0x70 + 0xC0 * n
}
const fn out_ch(n: usize) -> usize {
    BASE + 0xD0 + 0xC0 * n
}
const fn in_link_addr(n: usize) -> usize {
    BASE + 0x3AC + 4 * n
}
const fn out_link_addr(n: usize) -> usize {
    BASE + 0x3B8 + 4 * n
}

// Interrupt block offsets.
const INT_RAW: usize = 0x0;
const INT_ENA: usize = 0x8;
const INT_CLR: usize = 0xC;

// RX block (relative to in_ch).
const IN_CONF0: usize = 0x00;
const IN_RST: u32 = 1 << 0;
const INDSCR_BURST_EN: u32 = 1 << 2;
const IN_DATA_BURST_EN: u32 = 1 << 3;
const IN_CONF1: usize = 0x04;
const IN_CHECK_OWNER: u32 = 1 << 12;
const IN_LINK: usize = 0x10;
const INLINK_STOP: u32 = 1 << 1;
const INLINK_START: u32 = 1 << 2;
const IN_SUC_EOF_DES_ADDR: usize = 0x18;
const IN_DSCR: usize = 0x20;
const IN_PRI: usize = 0x2C;
const IN_PERI_SEL: usize = 0x30;

// TX block (relative to out_ch).
const OUT_CONF0: usize = 0x00;
const OUT_RST: u32 = 1 << 0;
const OUT_AUTO_WRBACK: u32 = 1 << 2;
const OUT_EOF_MODE: u32 = 1 << 3;
const OUTDSCR_BURST_EN: u32 = 1 << 4;
const OUT_DATA_BURST_EN: u32 = 1 << 5;
const OUT_CONF1: usize = 0x04;
const OUT_CHECK_OWNER: u32 = 1 << 12;
const OUT_LINK: usize = 0x10;
const OUTLINK_STOP: u32 = 1 << 0;
const OUTLINK_START: u32 = 1 << 1;
const OUT_EOF_DES_ADDR: usize = 0x18;
const OUT_DSCR: usize = 0x20;
const OUT_PRI: usize = 0x2C;
const OUT_PERI_SEL: usize = 0x30;

/// No peripheral (`AHB_DMA_LL_INVALID_PERIPH_ID`).
const PERI_NONE: u32 = 0x3F;

/// Peripheral selection values (`AHB_DMA_PERI_{IN,OUT}_SEL`).
pub mod peri {
    /// I2S0.
    pub const I2S0: u32 = 3;
    /// I2S1.
    pub const I2S1: u32 = 4;
    /// I2S2.
    pub const I2S2: u32 = 5;
}

/// Offset from an L2MEM address to its non-cacheable alias.
pub const UNCACHED_OFFSET: usize = 0x4000_0000;

/// The non-cacheable alias of the L2MEM address `addr`.
pub const fn uncached(addr: usize) -> usize {
    addr + UNCACHED_OFFSET
}

/// A GDMA link descriptor (`dma_descriptor_align4_t`): 12 bytes, 4-byte
/// aligned.
#[repr(C, align(4))]
#[derive(Clone, Copy, Debug, Default)]
pub struct Descriptor {
    /// size\[11:0\], length\[23:12\], err_eof\[28\], suc_eof\[30\],
    /// owner\[31\] (1 = DMA).
    pub dw0: u32,
    /// Buffer address.
    pub buf: u32,
    /// Next descriptor's address, 0 at the end of a list.
    pub next: u32,
}

impl Descriptor {
    /// Owner bit: set while the descriptor belongs to the DMA.
    pub const OWNER_DMA: u32 = 1 << 31;
    /// End of frame: on TX the DMA reports the descriptor at the EOF
    /// address register when done; on RX set by the DMA when the
    /// peripheral signalled EOF.
    pub const SUC_EOF: u32 = 1 << 30;
    /// Largest buffer one descriptor can carry.
    pub const MAX_SIZE: usize = 4095;

    /// `dw0` from its fields.
    pub const fn dw0(size: usize, length: usize, suc_eof: bool, owner_dma: bool) -> u32 {
        (size as u32 & 0xFFF)
            | ((length as u32 & 0xFFF) << 12)
            | if suc_eof { Self::SUC_EOF } else { 0 }
            | if owner_dma { Self::OWNER_DMA } else { 0 }
    }

    /// The `length` field of `dw0`.
    pub const fn length_of(dw0: u32) -> usize {
        ((dw0 >> 12) & 0xFFF) as usize
    }
}

static mut READY: bool = false;

/// Turns the AHB GDMA's clock on and resets it (once; later calls do
/// nothing). Channels in use by others would be disturbed by the reset, so
/// it is done only the first time.
pub fn init() {
    if unsafe { READY } {
        return;
    }
    set_bits(SOC_CLK_CTRL1, AHB_PDMA_SYS_CLK_EN);
    set_bits(HP_RST_EN1, RST_EN_AHB_PDMA);
    clear_bits(HP_RST_EN1, RST_EN_AHB_PDMA);
    // ahb_dma_ll_force_enable_reg_clock, ahb_dma_ll_reset_fsm,
    // ahb_dma_ll_set_default_memory_range.
    set_bits(MISC_CONF, MISC_CLK_EN);
    set_bits(MISC_CONF, MISC_AHBM_RST_INTER);
    clear_bits(MISC_CONF, MISC_AHBM_RST_INTER);
    unsafe {
        wr(INTR_MEM_START, 0x4000_0000);
        wr(INTR_MEM_END, 0x4FFC_0000);
        READY = true;
    }
}

/// TX (memory to peripheral) direction of channel `n` (0..=2).
pub struct TxChannel(usize);

/// RX (peripheral to memory) direction of channel `n` (0..=2).
pub struct RxChannel(usize);

impl TxChannel {
    /// The TX direction of channel `n`.
    ///
    /// # Safety
    /// Nothing else may use it meanwhile.
    pub unsafe fn steal(n: usize) -> Self {
        assert!(n < 3);
        TxChannel(n)
    }

    /// Stops and resets the channel and connects it to peripheral `peri`
    /// ([`peri`]): burst on, owner check off (the DMA never stops at a
    /// descriptor), `write_back` = descriptors written back (owner cleared)
    /// when done, EOF once the data has left the DMA FIFO; interrupts
    /// masked and cleared.
    pub fn configure(&self, peri: u32, write_back: bool) {
        let c = out_ch(self.0);
        unsafe {
            wr(c + OUT_LINK, OUTLINK_STOP);
            wr(c + OUT_PERI_SEL, PERI_NONE);
            set_bits(c + OUT_CONF0, OUT_RST);
            clear_bits(c + OUT_CONF0, OUT_RST);
            let wb = if write_back { OUT_AUTO_WRBACK } else { 0 };
            wr(c + OUT_CONF0, wb | OUT_EOF_MODE | OUTDSCR_BURST_EN | OUT_DATA_BURST_EN);
            clear_bits(c + OUT_CONF1, OUT_CHECK_OWNER);
            wr(c + OUT_PRI, 0);
            wr(out_int(self.0) + INT_ENA, 0);
            wr(out_int(self.0) + INT_CLR, 0x3F);
            wr(c + OUT_PERI_SEL, peri);
        }
    }

    /// Starts on the list at `desc` (an L2MEM address).
    pub fn start(&self, desc: usize) {
        unsafe {
            wr(out_link_addr(self.0), desc as u32);
            wr(out_ch(self.0) + OUT_LINK, OUTLINK_START);
        }
    }

    /// Stops the channel and disconnects it.
    pub fn stop(&self) {
        let c = out_ch(self.0);
        unsafe {
            wr(c + OUT_LINK, OUTLINK_STOP);
            wr(c + OUT_PERI_SEL, PERI_NONE);
        }
    }

    /// The descriptor last finished with `suc_eof` set
    /// (`OUT_EOF_DES_ADDR`).
    pub fn eof_desc_addr(&self) -> usize {
        unsafe { rd(out_ch(self.0) + OUT_EOF_DES_ADDR) as usize }
    }

    /// The descriptor the channel is working on (`OUT_DSCR`).
    pub fn current_desc_addr(&self) -> usize {
        unsafe { rd(out_ch(self.0) + OUT_DSCR) as usize }
    }

    /// Raw interrupt bits (`OUT_INT_RAW`: done, eof, dscr_err, total_eof,
    /// fifo_ovf, fifo_udf).
    pub fn int_raw(&self) -> u32 {
        unsafe { rd(out_int(self.0) + INT_RAW) }
    }

    /// Clears raw interrupt bits.
    pub fn clear_int(&self, bits: u32) {
        unsafe { wr(out_int(self.0) + INT_CLR, bits) }
    }
}

impl RxChannel {
    /// The RX direction of channel `n`.
    ///
    /// # Safety
    /// Nothing else may use it meanwhile.
    pub unsafe fn steal(n: usize) -> Self {
        assert!(n < 3);
        RxChannel(n)
    }

    /// Stops and resets the channel and connects it to peripheral `peri`
    /// ([`peri`]): burst on, owner check off; interrupts masked and
    /// cleared.
    pub fn configure(&self, peri: u32) {
        let c = in_ch(self.0);
        unsafe {
            wr(c + IN_LINK, INLINK_STOP);
            wr(c + IN_PERI_SEL, PERI_NONE);
            set_bits(c + IN_CONF0, IN_RST);
            clear_bits(c + IN_CONF0, IN_RST);
            wr(c + IN_CONF0, INDSCR_BURST_EN | IN_DATA_BURST_EN);
            clear_bits(c + IN_CONF1, IN_CHECK_OWNER);
            wr(c + IN_PRI, 0);
            wr(in_int(self.0) + INT_ENA, 0);
            wr(in_int(self.0) + INT_CLR, 0x7F);
            wr(c + IN_PERI_SEL, peri);
        }
    }

    /// Starts on the list at `desc` (an L2MEM address).
    pub fn start(&self, desc: usize) {
        unsafe {
            wr(in_link_addr(self.0), desc as u32);
            wr(in_ch(self.0) + IN_LINK, INLINK_START);
        }
    }

    /// Stops the channel and disconnects it.
    pub fn stop(&self) {
        let c = in_ch(self.0);
        unsafe {
            wr(c + IN_LINK, INLINK_STOP);
            wr(c + IN_PERI_SEL, PERI_NONE);
        }
    }

    /// The descriptor last finished on an EOF (`IN_SUC_EOF_DES_ADDR`).
    pub fn suc_eof_desc_addr(&self) -> usize {
        unsafe { rd(in_ch(self.0) + IN_SUC_EOF_DES_ADDR) as usize }
    }

    /// The descriptor the channel is working on (`IN_DSCR`).
    pub fn current_desc_addr(&self) -> usize {
        unsafe { rd(in_ch(self.0) + IN_DSCR) as usize }
    }

    /// Raw interrupt bits (`IN_INT_RAW`: done, suc_eof, err_eof, dscr_err,
    /// dscr_empty, fifo_ovf, fifo_udf).
    pub fn int_raw(&self) -> u32 {
        unsafe { rd(in_int(self.0) + INT_RAW) }
    }

    /// Clears raw interrupt bits.
    pub fn clear_int(&self, bits: u32) {
        unsafe { wr(in_int(self.0) + INT_CLR, bits) }
    }
}
