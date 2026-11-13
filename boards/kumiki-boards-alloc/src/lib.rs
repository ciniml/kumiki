//! # kumiki-boards-alloc
//!
//! Bring up a kumiki board (ESP32-S3) with its DMA buffers taken from an
//! allocator, sized for the board that was detected (doc/15 §3). This is the
//! only kumiki crate that allocates; it does so once, at bring-up, and the
//! buffers are kept for the program's lifetime.
//!
//! ```ignore
//! // esp-alloc's heap (feature `esp-alloc`):
//! esp_alloc::heap_allocator!(size: 72 * 1024);
//! let mut dev = kumiki_boards_alloc::init_from_heap(peripherals)?;
//! // The PSRAM the board's devices left over becomes heap too:
//! if let Some(rest) = dev.psram.take() {
//!     kumiki_boards_alloc::add_psram_to_heap(rest);
//! }
//! let sprite = Vec::with_capacity_in(w * h * 2, esp_alloc::ExternalMemory);
//!
//! // Any allocator-api2 allocator of DMA-capable internal RAM:
//! let dev = kumiki_boards_alloc::init_in(peripherals, &MY_DMA_ARENA)?;
//! ```
#![no_std]
#![warn(missing_docs)]

use allocator_api2::alloc::{Allocator, Layout};
use esp_hal::{
    dma::{BurstConfig, DmaDescriptor, DmaRxStreamBuf, DmaTxBuf, DmaTxStreamBuf, aligned::DmaAlignedMut, descriptor_count},
    peripherals::Peripherals,
};
use kumiki_boards::{BufferNeeds, Buffers, Devices, InitError};

/// Bring-up error.
#[derive(Debug)]
pub enum Error {
    /// The allocator could not supply this buffer.
    NoMemory(&'static str),
    /// The allocator's memory is not usable by DMA (wrong region or alignment).
    NotDmaCapable(&'static str),
    /// Detection or bring-up failed.
    Init(InitError),
}

impl From<InitError> for Error {
    fn from(e: InitError) -> Self {
        Error::Init(e)
    }
}

/// Detect the board and bring it up with the DMA buffers it needs
/// ([`kumiki_boards::buffer_needs`]) taken from `dma`, which must hand out
/// DMA-capable internal RAM. Call once, after `esp_hal::init`.
///
/// The buffers are never freed, hence the `'static` allocator.
pub fn init_in<A: Allocator>(mut p: Peripherals, dma: &'static A) -> Result<Devices, Error> {
    let board = kumiki_boards::detect(&mut p)?;
    let bufs = alloc_buffers(&kumiki_boards::buffer_needs(board), dma)?;
    Ok(kumiki_boards::init_with(p, board, bufs)?)
}

/// [`init_in`] on esp-alloc's heap (DMA-capable internal memory).
#[cfg(feature = "esp-alloc")]
pub fn init_from_heap(p: Peripherals) -> Result<Devices, Error> {
    init_in(p, &esp_alloc::DmaCompatibleInternalMemory)
}

/// Add `region` (e.g. `Devices.psram`) to esp-alloc's heap as external
/// memory: `Vec::new_in(esp_alloc::ExternalMemory)` and the global allocator
/// can then use it.
#[cfg(feature = "esp-alloc")]
pub fn add_psram_to_heap(region: &'static mut [u8]) {
    // SAFETY: `'static` and exclusive (taken by `&mut`), so the heap owns it from now on.
    unsafe {
        esp_alloc::HEAP.add_region(esp_alloc::HeapRegion::new(
            region.as_mut_ptr(),
            region.len(),
            esp_alloc::MemoryCapability::External.into(),
        ));
    }
}

/// Allocate the buffers `need` lists from `dma`.
pub fn alloc_buffers<A: Allocator>(need: &BufferNeeds, dma: &'static A) -> Result<Buffers, Error> {
    let mut bufs = Buffers::default();
    if need.display > 0 {
        let one = || -> Result<DmaTxBuf, Error> {
            let chunk = BurstConfig::DEFAULT.max_compatible_chunk_size();
            let (d, b) = parts(dma, need.display, descriptor_count(need.display, chunk), "display")?;
            DmaTxBuf::new(d, b).map_err(|_| Error::NotDmaCapable("display"))
        };
        bufs.display = Some((one()?, one()?));
    }
    if need.i2s_tx.0 > 0 {
        let (d, b) = parts(dma, need.i2s_tx.0, ring_descriptors(need.i2s_tx), "i2s-tx")?;
        bufs.i2s_tx = Some(DmaTxStreamBuf::new(d, b).map_err(|_| Error::NotDmaCapable("i2s-tx"))?);
    }
    if need.i2s_rx.0 > 0 {
        let (d, b) = parts(dma, need.i2s_rx.0, ring_descriptors(need.i2s_rx), "i2s-rx")?;
        bufs.i2s_rx = Some(DmaRxStreamBuf::new(d, b).map_err(|_| Error::NotDmaCapable("i2s-rx"))?);
    }
    Ok(bufs)
}

/// Descriptors of a ring of (bytes, bytes per descriptor); esp-hal wants at least 4.
fn ring_descriptors((len, chunk): (usize, usize)) -> usize {
    descriptor_count(len, chunk.max(1)).max(4)
}

/// `n` descriptors and a `len`-byte buffer from `a`, checked for DMA.
fn parts<A: Allocator>(
    a: &'static A,
    len: usize,
    n: usize,
    what: &'static str,
) -> Result<(DmaAlignedMut<'static, [DmaDescriptor]>, DmaAlignedMut<'static, [u8]>), Error> {
    let desc = Layout::array::<DmaDescriptor>(n).map_err(|_| Error::NoMemory(what))?;
    let p = a.allocate(desc).map_err(|_| Error::NoMemory(what))?.cast::<DmaDescriptor>();
    // SAFETY: a fresh allocation of `n` descriptors, initialised before use and never freed.
    let desc = unsafe {
        for i in 0..n {
            p.as_ptr().add(i).write(DmaDescriptor::EMPTY);
        }
        core::slice::from_raw_parts_mut(p.as_ptr(), n)
    };
    let buf = Layout::from_size_align(len, 4).map_err(|_| Error::NoMemory(what))?;
    let b = a.allocate_zeroed(buf).map_err(|_| Error::NoMemory(what))?;
    // SAFETY: a fresh, zeroed allocation of `len` bytes, never freed.
    let buf = unsafe { &mut *b.as_ptr() };
    let desc = DmaAlignedMut::new(desc).map_err(|_| Error::NotDmaCapable(what))?;
    let buf = DmaAlignedMut::new(buf).map_err(|_| Error::NotDmaCapable(what))?;
    Ok((desc, buf))
}
