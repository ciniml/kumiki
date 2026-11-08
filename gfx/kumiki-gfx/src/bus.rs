//! Panel bus traits.
//!
//! A *bus* moves bytes to a panel and knows nothing about the panel's command
//! set. The split mirrors M5GFX's `IBus`: the panel decides *what* to send
//! (window commands, init tables) and the bus decides *how* (SPI + DMA, i8080,
//! ...). Pixel conversion is handed to the bus through [`blocking::PanelBus::write_with`],
//! which lets a DMA-capable bus convert chunk N+1 while chunk N is on the wire.
//!
//! Both a blocking and an `async` flavour exist. Neither depends on any
//! executor or time driver.

/// Common error type declaration for buses.
pub trait ErrorType {
    /// Bus error.
    type Error: core::fmt::Debug;
}

/// Blocking bus API.
pub mod blocking {
    use super::ErrorType;

    /// A command/data bus to a MIPI-DCS-like panel.
    ///
    /// After [`command`](PanelBus::command) the bus is in the data phase:
    /// subsequent `write_*` calls stream data bytes (e.g. pixels after RAMWR).
    pub trait PanelBus: ErrorType {
        /// Send `cmd` (D/C low) followed by `params` (D/C high).
        fn command(&mut self, cmd: u8, params: &[u8]) -> Result<(), Self::Error>;

        /// Stream data bytes. When this returns, `data` may be reused.
        fn write_data(&mut self, data: &[u8]) -> Result<(), Self::Error> {
            let mut off = 0;
            self.write_with(data.len(), 1, |buf| {
                buf.copy_from_slice(&data[off..off + buf.len()]);
                off += buf.len();
            })
        }

        /// Stream `pattern` repeated `count` times (used for solid fills).
        fn write_repeat(&mut self, pattern: &[u8], count: u32) -> Result<(), Self::Error> {
            let total = pattern.len() * count as usize;
            let unit = pattern.len();
            self.write_with(total, unit, |buf| {
                for chunk in buf.chunks_exact_mut(unit) {
                    chunk.copy_from_slice(pattern);
                }
            })
        }

        /// Pipelined streaming: the bus repeatedly hands out a scratch buffer
        /// whose length is a non-zero multiple of `unit` and at most the
        /// remaining byte count; `fill` must fill it completely. Implementations
        /// may transmit the previous buffer while `fill` runs (DMA double
        /// buffering). Exactly `total` bytes are produced overall.
        fn write_with<F>(&mut self, total: usize, unit: usize, fill: F) -> Result<(), Self::Error>
        where
            F: FnMut(&mut [u8]);

        /// Wait until every queued byte has left the bus.
        fn flush(&mut self) -> Result<(), Self::Error>;
    }
}

/// `async` bus API (executor-agnostic).
pub mod asynch {
    use super::ErrorType;

    /// `async` flavour of [`super::blocking::PanelBus`]. `fill` stays synchronous:
    /// it is CPU work that overlaps with the in-flight transfer.
    #[allow(async_fn_in_trait)]
    pub trait PanelBus: ErrorType {
        /// Send `cmd` (D/C low) followed by `params` (D/C high).
        async fn command(&mut self, cmd: u8, params: &[u8]) -> Result<(), Self::Error>;

        /// Stream data bytes.
        async fn write_data(&mut self, data: &[u8]) -> Result<(), Self::Error> {
            let mut off = 0;
            self.write_with(data.len(), 1, |buf| {
                buf.copy_from_slice(&data[off..off + buf.len()]);
                off += buf.len();
            })
            .await
        }

        /// Stream `pattern` repeated `count` times.
        async fn write_repeat(&mut self, pattern: &[u8], count: u32) -> Result<(), Self::Error> {
            let total = pattern.len() * count as usize;
            let unit = pattern.len();
            self.write_with(total, unit, |buf| {
                for chunk in buf.chunks_exact_mut(unit) {
                    chunk.copy_from_slice(pattern);
                }
            })
            .await
        }

        /// Pipelined streaming (see [`super::blocking::PanelBus::write_with`]).
        async fn write_with<F>(&mut self, total: usize, unit: usize, fill: F) -> Result<(), Self::Error>
        where
            F: FnMut(&mut [u8]);

        /// Wait until every queued byte has left the bus.
        async fn flush(&mut self) -> Result<(), Self::Error>;
    }
}

/// Split `total` bytes into chunks for a scratch buffer of `cap` bytes, keeping
/// each chunk a multiple of `unit`. The first chunks are small and grow
/// geometrically (as in M5GFX's `writePixels`) so the first transfer starts early.
#[derive(Clone, Copy, Debug)]
pub struct ChunkPlan {
    remaining: usize,
    unit: usize,
    cap: usize,
    next: usize,
}

impl ChunkPlan {
    /// Create a plan. `cap` must be at least `unit`.
    pub fn new(total: usize, unit: usize, cap: usize) -> Self {
        let unit = unit.max(1);
        let cap = (cap / unit * unit).max(unit);
        let first = (unit * 32).clamp(unit, cap);
        Self { remaining: total, unit, cap, next: first }
    }
}

impl Iterator for ChunkPlan {
    type Item = usize;
    fn next(&mut self) -> Option<usize> {
        if self.remaining == 0 {
            return None;
        }
        let n = self.next.min(self.remaining);
        // `remaining` is always a multiple of unit for well-formed callers; if
        // not, the last chunk simply carries the tail.
        self.remaining -= n;
        self.next = (self.next * 2).min(self.cap) / self.unit * self.unit;
        Some(n)
    }
}

#[cfg(test)]
mod tests {
    use super::ChunkPlan;

    #[test]
    fn chunk_plan_grows_and_covers_total() {
        let v: Vec<usize> = ChunkPlan::new(10_000, 2, 4096).collect();
        assert_eq!(v.iter().sum::<usize>(), 10_000);
        assert_eq!(v[0], 64);
        assert!(v.iter().all(|n| n % 2 == 0 && *n <= 4096));
    }
}
