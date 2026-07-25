//! dither-core — 8x8 ordered (Bayer) dithering + level quantization.
//!
//! This is the shared "pixel-art" tone map: it kills gradient banding while
//! staying crisp under motion. Previously copy-pasted into all six celestial
//! crates; the matrix and math are unchanged, so output is identical.

use noise_core::{clamp01, Rgb};

// 8x8 ordered (Bayer) matrix, values 0..63.
pub const BAYER: [u8; 64] = [
    0, 32, 8, 40, 2, 34, 10, 42, 48, 16, 56, 24, 50, 18, 58, 26, 12, 44, 4, 36, 14, 46,
    6, 38, 60, 28, 52, 20, 62, 30, 54, 22, 3, 35, 11, 43, 1, 33, 9, 41, 51, 19, 59, 27,
    49, 17, 57, 25, 15, 47, 7, 39, 13, 45, 5, 37, 63, 31, 55, 23, 61, 29, 53, 21,
];

/// Ordered-dither bias for pixel `(x, y)`, in -0.5..0.5.
pub fn bayer(x: u32, y: u32) -> f32 {
    (BAYER[((y % 8) * 8 + (x % 8)) as usize] as f32 + 0.5) / 64.0 - 0.5
}

/// Ordered-dither quantize to `levels` steps. `bx` is the per-pixel Bayer bias
/// (from [`bayer`]); `dither` scales its strength (typically 0.7). `levels`
/// varies by crate: 22 (planet/star/asteroid) or 24 (solar/moon/comet).
pub fn quant(o: Rgb, bx: f32, levels: f32, dither: f32) -> Rgb {
    let d = bx * dither / levels;
    [
        clamp01(((o[0] + d) * levels).round() / levels),
        clamp01(((o[1] + d) * levels).round() / levels),
        clamp01(((o[2] + d) * levels).round() / levels),
    ]
}
