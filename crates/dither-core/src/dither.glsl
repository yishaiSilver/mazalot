// dither.glsl — the GLSL half of dither-core. Concatenated after
// `noise_core::GL_PRELUDE`, so it carries no `#version` of its own.

const int BAYER[64] = int[64](
   0, 32,  8, 40,  2, 34, 10, 42, 48, 16, 56, 24, 50, 18, 58, 26,
  12, 44,  4, 36, 14, 46,  6, 38, 60, 28, 52, 20, 62, 30, 54, 22,
   3, 35, 11, 43,  1, 33,  9, 41, 51, 19, 59, 27, 49, 17, 57, 25,
  15, 47,  7, 39, 13, 45,  5, 37, 63, 31, 55, 23, 61, 29, 53, 21);

/// Ordered-dither bias for pixel `(x, y)`, in -0.5..0.5. `%` on a u32 in the
/// Rust; the coordinates here are never negative, so `&7` is the same value.
float bayer(int x, int y) { return (float(BAYER[(y & 7) * 8 + (x & 7)]) + 0.5) / 64.0 - 0.5; }

/// Ordered-dither quantize to `levels` steps — 22 for planet/star, 24 for the
/// scene crates.
///
/// `floor(v + 0.5)` rather than GLSL `round()`, whose behaviour at exactly .5 is
/// implementation-defined; Rust's `round` is half-away-from-zero, and below zero
/// the two agree once the clamp has run.
vec3 quant(vec3 o, float bx, float levels, float dither) {
  float d = bx * dither / levels;
  return clamp(floor((o + vec3(d)) * levels + 0.5) / levels, 0.0, 1.0);
}

/// The Rust truncates onto the 0..255 scale rather than rounding, and a canvas
/// rounds on the way in — so floor here, and the byte matches exactly.
vec3 toByte(vec3 c) { return floor(clamp(c, 0.0, 1.0) * 255.0) / 255.0; }
