//! WASM glue for the browser demo. Raw C ABI — no wasm-bindgen.
//!
//! All planet generation lives in `planet-core`; this file only exposes a
//! pointer-based interface JavaScript can call:
//!   1. `alloc(len)` -> a buffer pointer in wasm linear memory
//!   2. `render(ptr, size, type_idx, seed, angle)` -> fills it with RGBA
//!   3. read the bytes back from `memory.buffer`, draw to a canvas
//!   4. `dealloc(ptr, len)` when done

use std::slice;

// The byte-identical `alloc`/`dealloc` C-ABI pair, emitted in-crate.
wasm_abi::alloc_dealloc!();

/// Render a planet frame (RGBA) into the buffer at `ptr`.
#[no_mangle]
pub extern "C" fn render(ptr: *mut u8, size: u32, type_idx: u32, seed: u32, angle: f32) {
    let out = unsafe { slice::from_raw_parts_mut(ptr, (size * size * 4) as usize) };
    crate::render_rgba(size, type_idx as usize, seed, angle, out);
}

/// Render a planet with slider-overridden parameters.
#[no_mangle]
pub extern "C" fn render_custom(
    ptr: *mut u8,
    size: u32,
    type_idx: u32,
    seed: u32,
    angle: f32,
    contrast: f32,
    freq: f32,
    specular: f32,
    shininess: f32,
) {
    let out = unsafe { slice::from_raw_parts_mut(ptr, (size * size * 4) as usize) };
    crate::render_rgba_custom(
        size, type_idx as usize, seed, angle, contrast, freq, specular, shininess, out,
    );
}

/// Read a type's default value for parameter `which` (see crate::param),
/// so the sliders can snap to sensible per-type starting values.
#[no_mangle]
pub extern "C" fn param(type_idx: u32, which: u32) -> f32 {
    crate::param(type_idx as usize, which)
}

/// Number of tunable parameters (length of the array `render_params` expects).
#[no_mangle]
pub extern "C" fn num_params() -> u32 {
    crate::NUM_PARAMS as u32
}

/// Render with params + global style: `palette` (0 natural, 1 game boy, 2 ice,
/// 3 sunset), `dither` (0..1), `moons` (0/1).
#[no_mangle]
pub extern "C" fn render_styled(
    ptr: *mut u8,
    size: u32,
    type_idx: u32,
    seed: u32,
    angle: f32,
    params_ptr: *const f32,
    palette: u32,
    dither: f32,
    moons: u32,
) {
    let out = unsafe { slice::from_raw_parts_mut(ptr, (size * size * 4) as usize) };
    let params = unsafe { slice::from_raw_parts(params_ptr, crate::NUM_PARAMS) };
    crate::render_rgba_styled(size, type_idx as usize, seed, angle, params, palette, dither, moons, out);
}

/// Render with a full slider-parameter override array. `params_ptr` points at
/// `num_params()` f32 values in wasm memory (written by JS each frame).
#[no_mangle]
pub extern "C" fn render_params(
    ptr: *mut u8,
    size: u32,
    type_idx: u32,
    seed: u32,
    angle: f32,
    params_ptr: *const f32,
) {
    let out = unsafe { slice::from_raw_parts_mut(ptr, (size * size * 4) as usize) };
    let params = unsafe { slice::from_raw_parts(params_ptr, crate::NUM_PARAMS) };
    crate::render_rgba_params(size, type_idx as usize, seed, angle, params, out);
}

/// Render with the feature switches exposed — the ablation panel's entry point.
/// `features` is a mask of `planet_core`'s `F_*` bits; `feat_all()` is the
/// normal picture. Switching one bit off and timing the difference is how the
/// per-feature costs are measured.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn render_features(
    ptr: *mut u8,
    size: u32,
    type_idx: u32,
    seed: u32,
    angle: f32,
    params_ptr: *const f32,
    palette: u32,
    dither: f32,
    moons: u32,
    features: u32,
) {
    let out = unsafe { slice::from_raw_parts_mut(ptr, (size * size * 4) as usize) };
    let params = unsafe { slice::from_raw_parts(params_ptr, crate::NUM_PARAMS) };
    planet_core::render_rgba_features(
        size, type_idx as usize, seed, angle, params, palette, dither, moons, features, out,
    );
}

/// The all-features-on mask, so JS need not hard-code the bit count.
#[no_mangle]
pub extern "C" fn feat_all() -> u32 {
    planet_core::F_ALL
}

/// Number of planet types (for the JS "random type" picker).
#[no_mangle]
pub extern "C" fn type_count() -> u32 {
    crate::type_count() as u32
}

/// Render only rows `y0..y1` of the frame — the entry point a worker pool uses
/// to split one frame across several instances.
///
/// `ptr` is a WHOLE frame's worth of pixels; rows outside the band are left
/// untouched, so each worker owns its own buffer and the caller copies the band
/// back at its real offset. Concatenating the bands reproduces `render_features`
/// exactly (pinned by `render_band_matches_whole`).
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn render_band(
    ptr: *mut u8,
    size: u32,
    type_idx: u32,
    seed: u32,
    angle: f32,
    params_ptr: *const f32,
    palette: u32,
    dither: f32,
    moons: u32,
    features: u32,
    y0: u32,
    y1: u32,
) {
    let out = unsafe { slice::from_raw_parts_mut(ptr, (size * size * 4) as usize) };
    let params = unsafe { slice::from_raw_parts(params_ptr, crate::NUM_PARAMS) };
    planet_core::render_rgba_band(
        size, type_idx as usize, seed, angle, params, palette, dither, moons, features, y0, y1, out,
    );
}
