//! Generic MIPI-DCS window panel.

use core::marker::PhantomData;

use embedded_graphics_core::{geometry::Size, primitives::Rectangle};
use embedded_hal::digital::OutputPin;
use kumiki_gfx::{
    bus::{asynch::PanelBus as AsyncPanelBus, blocking::PanelBus, ErrorType},
    panel::{asynch::WindowPanel as AsyncWindowPanel, blocking::WindowPanel, Panel},
    Rgb565Be, Rotation,
};

use crate::models::{init_entries, Model};

const CASET: u8 = 0x2A;
const RASET: u8 = 0x2B;
const RAMWR: u8 = 0x2C;
const MADCTL: u8 = 0x36;
const COLMOD: u8 = 0x3A;
const INVOFF: u8 = 0x20;
const INVON: u8 = 0x21;
const SLPIN: u8 = 0x10;
const SLPOUT: u8 = 0x11;

const MAD_MY: u8 = 0x80;
const MAD_MX: u8 = 0x40;
const MAD_MV: u8 = 0x20;
const MAD_ML: u8 = 0x10;
const MAD_BGR: u8 = 0x08;
const MAD_MH: u8 = 0x04;

/// MADCTL per internal rotation (0-3 rotations, 4-7 mirrored). From M5GFX `Panel_LCD::getMadCtl`.
const MADCTL_TABLE: [u8; 8] = [
    0,
    MAD_MV | MAD_MX | MAD_MH,
    MAD_MX | MAD_MH | MAD_MY | MAD_ML,
    MAD_MV | MAD_MY | MAD_ML,
    MAD_MY | MAD_ML,
    MAD_MV,
    MAD_MX | MAD_MH,
    MAD_MV | MAD_MX | MAD_MY | MAD_MH | MAD_ML,
];

/// Board-level panel parameters (normally generated from the device tree).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DcsConfig {
    /// Visible size (width, height) in the controller's native orientation.
    pub panel_size: (u16, u16),
    /// Offset of the visible area inside controller memory (x, y), native orientation.
    pub offset: (u16, u16),
    /// Rotation (quarter turns, bit 2 = mirror) added so that [`Rotation::Deg0`]
    /// is the board's natural orientation.
    pub offset_rotation: u8,
    /// Colour inversion (INVON).
    pub invert: bool,
    /// `true`: RGB order, `false`: BGR order (MADCTL.BGR set).
    pub rgb_order: bool,
}

impl DcsConfig {
    /// Defaults of model `M`.
    pub const fn for_model<M: Model>() -> Self {
        Self { panel_size: M::PANEL_SIZE, offset: (0, 0), offset_rotation: 0, invert: M::INVERT, rgb_order: false }
    }
}

/// Error from panel control operations.
#[derive(Debug)]
pub enum PanelError<B, P> {
    /// Bus error.
    Bus(B),
    /// Reset pin error.
    Pin(P),
}

/// Placeholder for "no reset pin".
pub struct NoPin;

impl embedded_hal::digital::ErrorType for NoPin {
    type Error = core::convert::Infallible;
}
impl OutputPin for NoPin {
    fn set_low(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
    fn set_high(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}

/// A MIPI-DCS panel (model `M`) on bus `B`, with optional reset pin `RST`.
///
/// Writes RGB565 ([`Rgb565Be`]). Caches the CASET/RASET window like M5GFX's
/// `set_window_8`, so repeated writes to the same rectangle cost only RAMWR.
pub struct DcsPanel<B, M, RST = NoPin> {
    bus: B,
    cfg: DcsConfig,
    reset: Option<RST>,
    internal_rotation: u8,
    width: u16,
    height: u16,
    colstart: u16,
    rowstart: u16,
    cache_x: Option<(u16, u16)>,
    cache_y: Option<(u16, u16)>,
    _m: PhantomData<M>,
}

impl<B, M: Model, RST: OutputPin> DcsPanel<B, M, RST> {
    /// Create a panel. Call `init` / `init_async` before drawing.
    pub fn new(bus: B, _model: M, cfg: DcsConfig, reset: Option<RST>) -> Self {
        let mut p = Self {
            bus,
            cfg,
            reset,
            internal_rotation: 0,
            width: 0,
            height: 0,
            colstart: 0,
            rowstart: 0,
            cache_x: None,
            cache_y: None,
            _m: PhantomData,
        };
        p.apply_rotation(Rotation::Deg0);
        p
    }

    /// Release the bus and reset pin.
    pub fn release(self) -> (B, Option<RST>) {
        (self.bus, self.reset)
    }

    /// Board configuration in use.
    pub fn config(&self) -> &DcsConfig {
        &self.cfg
    }

    /// Recompute geometry for `r` (port of M5GFX `Panel_LCD::setRotation`).
    fn apply_rotation(&mut self, r: Rotation) {
        let r = r as u8;
        let off = self.cfg.offset_rotation;
        let ir = ((r + off) & 3) | ((r & 4) ^ (off & 4));
        self.internal_rotation = ir;
        let (mut ox, mut oy) = self.cfg.offset;
        let (mut pw, mut ph) = self.cfg.panel_size;
        let (mut mw, mut mh) = M::MEMORY_SIZE;
        if ir & 1 != 0 {
            core::mem::swap(&mut ox, &mut oy);
            core::mem::swap(&mut pw, &mut ph);
            core::mem::swap(&mut mw, &mut mh);
        }
        self.width = pw;
        self.height = ph;
        self.colstart = if ir & 2 != 0 { mw - (pw + ox) } else { ox };
        self.rowstart = if (1u8 << ir) & 0b1001_0110 != 0 { mh - (ph + oy) } else { oy };
        self.cache_x = None;
        self.cache_y = None;
    }

    fn madctl(&self) -> u8 {
        MADCTL_TABLE[self.internal_rotation as usize & 7] | if self.cfg.rgb_order { 0 } else { MAD_BGR }
    }

    /// Compute the CASET/RASET payloads for `area`; returns which ones changed.
    fn window_params(&mut self, area: &Rectangle) -> (Option<[u8; 4]>, Option<[u8; 4]>) {
        let xs = area.top_left.x as u16 + self.colstart;
        let xe = xs + area.size.width as u16 - 1;
        let ys = area.top_left.y as u16 + self.rowstart;
        let ye = ys + area.size.height as u16 - 1;
        let be = |a: u16, b: u16| {
            let (a, b) = (a.to_be_bytes(), b.to_be_bytes());
            [a[0], a[1], b[0], b[1]]
        };
        let x = (self.cache_x != Some((xs, xe))).then(|| {
            self.cache_x = Some((xs, xe));
            be(xs, xe)
        });
        let y = (self.cache_y != Some((ys, ye))).then(|| {
            self.cache_y = Some((ys, ye));
            be(ys, ye)
        });
        (x, y)
    }

    fn swapped(&self) -> bool {
        self.internal_rotation & 1 != 0
    }
}

// ---- blocking --------------------------------------------------------------

type BErr<B> = <B as ErrorType>::Error;

impl<B: PanelBus, M: Model, RST: OutputPin> DcsPanel<B, M, RST> {
    /// Hardware reset (if a pin is present) + init table + pixel format/rotation/inversion.
    pub fn init(&mut self, delay: &mut impl embedded_hal::delay::DelayNs) -> Result<(), PanelError<BErr<B>, RST::Error>> {
        if let Some(rst) = self.reset.as_mut() {
            rst.set_low().map_err(PanelError::Pin)?;
            delay.delay_ms(10);
            rst.set_high().map_err(PanelError::Pin)?;
            delay.delay_ms(120);
        }
        for (cmd, args, ms) in init_entries(M::INIT) {
            self.bus.command(cmd, args).map_err(PanelError::Bus)?;
            if ms > 0 {
                self.bus.flush().map_err(PanelError::Bus)?;
                delay.delay_ms(ms as u32);
            }
        }
        self.bus.command(COLMOD, &[0x55]).map_err(PanelError::Bus)?;
        let inv = if self.cfg.invert { INVON } else { INVOFF };
        self.bus.command(inv, &[]).map_err(PanelError::Bus)?;
        let m = self.madctl();
        self.bus.command(MADCTL, &[m]).map_err(PanelError::Bus)?;
        self.bus.flush().map_err(PanelError::Bus)
    }

    /// Change the logical rotation.
    pub fn set_rotation(&mut self, r: Rotation) -> Result<(), BErr<B>> {
        self.apply_rotation(r);
        let m = self.madctl();
        self.bus.command(MADCTL, &[m])
    }

    /// Enter/leave sleep mode.
    pub fn sleep(&mut self, on: bool) -> Result<(), BErr<B>> {
        self.bus.command(if on { SLPIN } else { SLPOUT }, &[])
    }
}

impl<B, M: Model, RST: OutputPin> Panel for DcsPanel<B, M, RST> {
    type Format = Rgb565Be;
    fn size(&self) -> Size {
        Size::new(self.width as u32, self.height as u32)
    }
}

impl<B: PanelBus, M: Model, RST: OutputPin> WindowPanel for DcsPanel<B, M, RST> {
    type Bus = B;

    fn begin_write(&mut self, area: &Rectangle) -> Result<(), BErr<B>> {
        let (x, y) = self.window_params(area);
        if x.is_some() && M::RESET_RASET_WHEN_SWAPPED && self.swapped() {
            self.bus.command(RASET, &[0xFF; 4])?;
            self.cache_y = None;
        }
        if let Some(p) = x {
            self.bus.command(CASET, &p)?;
        }
        let y = y.or_else(|| self.cache_y.is_none().then(|| self.window_params(area).1).flatten());
        if let Some(p) = y {
            self.bus.command(RASET, &p)?;
        }
        self.bus.command(RAMWR, &[])
    }

    fn bus(&mut self) -> &mut B {
        &mut self.bus
    }
}

// ---- async -----------------------------------------------------------------

impl<B: AsyncPanelBus, M: Model, RST: OutputPin> DcsPanel<B, M, RST> {
    /// `async` version of `init`.
    pub async fn init_async(
        &mut self,
        delay: &mut impl embedded_hal_async::delay::DelayNs,
    ) -> Result<(), PanelError<BErr<B>, RST::Error>> {
        if let Some(rst) = self.reset.as_mut() {
            rst.set_low().map_err(PanelError::Pin)?;
            delay.delay_ms(10).await;
            rst.set_high().map_err(PanelError::Pin)?;
            delay.delay_ms(120).await;
        }
        for (cmd, args, ms) in init_entries(M::INIT) {
            self.bus.command(cmd, args).await.map_err(PanelError::Bus)?;
            if ms > 0 {
                self.bus.flush().await.map_err(PanelError::Bus)?;
                delay.delay_ms(ms as u32).await;
            }
        }
        self.bus.command(COLMOD, &[0x55]).await.map_err(PanelError::Bus)?;
        let inv = if self.cfg.invert { INVON } else { INVOFF };
        self.bus.command(inv, &[]).await.map_err(PanelError::Bus)?;
        let m = self.madctl();
        self.bus.command(MADCTL, &[m]).await.map_err(PanelError::Bus)?;
        self.bus.flush().await.map_err(PanelError::Bus)
    }

    /// `async` version of `set_rotation`.
    pub async fn set_rotation_async(&mut self, r: Rotation) -> Result<(), BErr<B>> {
        self.apply_rotation(r);
        let m = self.madctl();
        self.bus.command(MADCTL, &[m]).await
    }
}

impl<B: AsyncPanelBus, M: Model, RST: OutputPin> AsyncWindowPanel for DcsPanel<B, M, RST> {
    type Bus = B;

    async fn begin_write(&mut self, area: &Rectangle) -> Result<(), BErr<B>> {
        let (x, y) = self.window_params(area);
        if x.is_some() && M::RESET_RASET_WHEN_SWAPPED && self.swapped() {
            self.bus.command(RASET, &[0xFF; 4]).await?;
            self.cache_y = None;
        }
        if let Some(p) = x {
            self.bus.command(CASET, &p).await?;
        }
        let y = y.or_else(|| self.cache_y.is_none().then(|| self.window_params(area).1).flatten());
        if let Some(p) = y {
            self.bus.command(RASET, &p).await?;
        }
        self.bus.command(RAMWR, &[]).await
    }

    fn bus(&mut self) -> &mut B {
        &mut self.bus
    }
}
