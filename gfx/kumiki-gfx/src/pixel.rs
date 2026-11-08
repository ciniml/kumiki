//! Pixel formats and conversion.
//!
//! Every format is described by its *wire* byte layout, i.e. the exact bytes a
//! panel (or a scan-out engine) consumes. Sprites and frame buffers store pixels
//! in this layout so that pushing them to a panel needs no conversion at all —
//! the same idea as M5GFX's `swap565_t` storage.

use embedded_graphics_core::pixelcolor::{
    raw::RawU16,
    IntoStorage, PixelColor, Rgb565, Rgb888, RgbColor,
};

/// Identifier used to select fast conversion paths at compile time.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FormatId {
    /// RGB565, big-endian (high byte first). Native order of SPI/i8080 MIPI-DCS panels.
    Rgb565Be,
    /// RGB565, little-endian. Native order of DPI/DSI frame buffers read by DMA.
    Rgb565Le,
    /// RGB888, bytes in R, G, B order.
    Rgb888,
}

/// A byte-aligned pixel format described by its wire layout.
pub trait PixelFormat: Copy + Default + 'static {
    /// Compile-time identifier (used to select fast paths).
    const ID: FormatId;
    /// Bytes per pixel.
    const BYTES: usize;
    /// The embedded-graphics colour type that maps 1:1 onto this format.
    type Color: PixelColor;

    /// Encode `color` into `out[..Self::BYTES]`.
    fn encode(color: Self::Color, out: &mut [u8]);
    /// Decode one pixel from `bytes[..Self::BYTES]`.
    fn decode(bytes: &[u8]) -> Self::Color;
    /// Convert one pixel to 8-bit-per-channel RGB (generic conversion path).
    fn to_rgb888(bytes: &[u8]) -> [u8; 3];
    /// Encode one pixel from 8-bit-per-channel RGB (generic conversion path).
    fn from_rgb888(rgb: [u8; 3], out: &mut [u8]);
}

/// RGB565, big-endian on the wire.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Rgb565Be;
/// RGB565, little-endian on the wire.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Rgb565Le;
/// RGB888 (R, G, B byte order).
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Rgb888Fmt;

#[inline(always)]
fn rgb565_from_888(rgb: [u8; 3]) -> u16 {
    ((rgb[0] as u16 & 0xF8) << 8) | ((rgb[1] as u16 & 0xFC) << 3) | (rgb[2] as u16 >> 3)
}

#[inline(always)]
fn rgb888_from_565(v: u16) -> [u8; 3] {
    let r = ((v >> 11) & 0x1F) as u8;
    let g = ((v >> 5) & 0x3F) as u8;
    let b = (v & 0x1F) as u8;
    [(r << 3) | (r >> 2), (g << 2) | (g >> 4), (b << 3) | (b >> 2)]
}

impl PixelFormat for Rgb565Be {
    const ID: FormatId = FormatId::Rgb565Be;
    const BYTES: usize = 2;
    type Color = Rgb565;
    #[inline(always)]
    fn encode(color: Rgb565, out: &mut [u8]) {
        out[..2].copy_from_slice(&color.into_storage().to_be_bytes());
    }
    #[inline(always)]
    fn decode(bytes: &[u8]) -> Rgb565 {
        RawU16::new(u16::from_be_bytes([bytes[0], bytes[1]])).into()
    }
    #[inline(always)]
    fn to_rgb888(bytes: &[u8]) -> [u8; 3] {
        rgb888_from_565(u16::from_be_bytes([bytes[0], bytes[1]]))
    }
    #[inline(always)]
    fn from_rgb888(rgb: [u8; 3], out: &mut [u8]) {
        out[..2].copy_from_slice(&rgb565_from_888(rgb).to_be_bytes());
    }
}

impl PixelFormat for Rgb565Le {
    const ID: FormatId = FormatId::Rgb565Le;
    const BYTES: usize = 2;
    type Color = Rgb565;
    #[inline(always)]
    fn encode(color: Rgb565, out: &mut [u8]) {
        out[..2].copy_from_slice(&color.into_storage().to_le_bytes());
    }
    #[inline(always)]
    fn decode(bytes: &[u8]) -> Rgb565 {
        RawU16::new(u16::from_le_bytes([bytes[0], bytes[1]])).into()
    }
    #[inline(always)]
    fn to_rgb888(bytes: &[u8]) -> [u8; 3] {
        rgb888_from_565(u16::from_le_bytes([bytes[0], bytes[1]]))
    }
    #[inline(always)]
    fn from_rgb888(rgb: [u8; 3], out: &mut [u8]) {
        out[..2].copy_from_slice(&rgb565_from_888(rgb).to_le_bytes());
    }
}

impl PixelFormat for Rgb888Fmt {
    const ID: FormatId = FormatId::Rgb888;
    const BYTES: usize = 3;
    type Color = Rgb888;
    #[inline(always)]
    fn encode(color: Rgb888, out: &mut [u8]) {
        out[0] = color.r();
        out[1] = color.g();
        out[2] = color.b();
    }
    #[inline(always)]
    fn decode(bytes: &[u8]) -> Rgb888 {
        Rgb888::new(bytes[0], bytes[1], bytes[2])
    }
    #[inline(always)]
    fn to_rgb888(bytes: &[u8]) -> [u8; 3] {
        [bytes[0], bytes[1], bytes[2]]
    }
    #[inline(always)]
    fn from_rgb888(rgb: [u8; 3], out: &mut [u8]) {
        out[..3].copy_from_slice(&rgb);
    }
}

/// Convert `pixels` pixels from `src` (format `S`) into `dst` (format `D`).
///
/// Panics if either slice is too short. Fast paths (selected at compile time):
/// identical formats → `copy_from_slice`; RGB565 BE↔LE → byte swap.
#[inline]
pub fn convert<S: PixelFormat, D: PixelFormat>(src: &[u8], dst: &mut [u8], pixels: usize) {
    let src = &src[..pixels * S::BYTES];
    let dst = &mut dst[..pixels * D::BYTES];
    if S::ID == D::ID {
        dst.copy_from_slice(src);
        return;
    }
    match (S::ID, D::ID) {
        (FormatId::Rgb565Be, FormatId::Rgb565Le) | (FormatId::Rgb565Le, FormatId::Rgb565Be) => {
            #[allow(clippy::chunks_exact_to_as_chunks)]
            for (d, s) in dst.chunks_exact_mut(2).zip(src.chunks_exact(2)) {
                d[0] = s[1];
                d[1] = s[0];
            }
        }
        _ => {
            for (d, s) in dst.chunks_exact_mut(D::BYTES).zip(src.chunks_exact(S::BYTES)) {
                D::from_rgb888(S::to_rgb888(s), d);
            }
        }
    }
}

/// Fill `dst` with `color` repeated (whole pixels only; trailing bytes untouched).
#[inline]
pub fn fill<F: PixelFormat>(dst: &mut [u8], color: F::Color) {
    let mut px = [0u8; 4];
    F::encode(color, &mut px);
    let px = &px[..F::BYTES];
    let whole = dst.len() / F::BYTES * F::BYTES;
    let dst = &mut dst[..whole];
    if dst.is_empty() {
        return;
    }
    // Seed one pixel, then grow by doubling copies (memcpy-speed fill).
    dst[..F::BYTES].copy_from_slice(px);
    let mut filled = F::BYTES;
    while filled < dst.len() {
        let n = filled.min(dst.len() - filled);
        dst.copy_within(..n, filled);
        filled += n;
    }
}

/// Encode a single colour into a small array (returns the array and its used length).
#[inline]
pub fn encode_one<F: PixelFormat>(color: F::Color) -> ([u8; 4], usize) {
    let mut px = [0u8; 4];
    F::encode(color, &mut px);
    (px, F::BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb565_be_roundtrip() {
        let c = Rgb565::new(31, 0, 7);
        let mut b = [0u8; 2];
        Rgb565Be::encode(c, &mut b);
        assert_eq!(b, [0xF8, 0x07]);
        assert_eq!(Rgb565Be::decode(&b), c);
    }

    #[test]
    fn convert_le_be_swaps() {
        let src = [0x12, 0x34, 0x56, 0x78];
        let mut dst = [0u8; 4];
        convert::<Rgb565Le, Rgb565Be>(&src, &mut dst, 2);
        assert_eq!(dst, [0x34, 0x12, 0x78, 0x56]);
    }

    #[test]
    fn convert_888_to_565() {
        let src = [0xFF, 0x00, 0x00, 0x00, 0xFF, 0x00];
        let mut dst = [0u8; 4];
        convert::<Rgb888Fmt, Rgb565Be>(&src, &mut dst, 2);
        assert_eq!(dst, [0xF8, 0x00, 0x07, 0xE0]);
    }

    #[test]
    fn fill_repeats_pattern() {
        let mut buf = [0u8; 9];
        fill::<Rgb565Be>(&mut buf, Rgb565::new(31, 63, 31));
        assert_eq!(&buf[..8], &[0xFF; 8]);
        assert_eq!(buf[8], 0);
    }
}
