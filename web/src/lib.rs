//! WASM glue for the browser demo. Raw C ABI — no wasm-bindgen.
//!
//! All planet generation lives in `planet-core`; this file only exposes a
//! pointer-based interface JavaScript can call:
//!   1. `alloc(len)` -> a buffer pointer in wasm linear memory
//!   2. `render(ptr, size, type_idx, seed, angle)` -> fills it with RGBA
//!   3. read the bytes back from `memory.buffer`, draw to a canvas
//!   4. `dealloc(ptr, len)` when done

use std::slice;

/// Allocate `len` bytes in wasm memory and hand the pointer to JS.
#[no_mangle]
pub extern "C" fn alloc(len: usize) -> *mut u8 {
    let mut v = Vec::<u8>::with_capacity(len);
    let ptr = v.as_mut_ptr();
    std::mem::forget(v);
    ptr
}

/// Free a buffer previously returned by `alloc`.
#[no_mangle]
pub extern "C" fn dealloc(ptr: *mut u8, len: usize) {
    if !ptr.is_null() {
        unsafe {
            drop(Vec::from_raw_parts(ptr, len, len));
        }
    }
}

/// Render a planet frame (RGBA) into the buffer at `ptr`.
#[no_mangle]
pub extern "C" fn render(ptr: *mut u8, size: u32, type_idx: u32, seed: u32, angle: f32) {
    let out = unsafe { slice::from_raw_parts_mut(ptr, (size * size * 4) as usize) };
    planet_core::render_rgba(size, type_idx as usize, seed, angle, out);
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
    planet_core::render_rgba_custom(
        size, type_idx as usize, seed, angle, contrast, freq, specular, shininess, out,
    );
}

/// Read a type's default value for parameter `which` (see planet_core::param),
/// so the sliders can snap to sensible per-type starting values.
#[no_mangle]
pub extern "C" fn param(type_idx: u32, which: u32) -> f32 {
    planet_core::param(type_idx as usize, which)
}

/// Number of tunable parameters (length of the array `render_params` expects).
#[no_mangle]
pub extern "C" fn num_params() -> u32 {
    planet_core::NUM_PARAMS as u32
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
    let params = unsafe { slice::from_raw_parts(params_ptr, planet_core::NUM_PARAMS) };
    planet_core::render_rgba_styled(size, type_idx as usize, seed, angle, params, palette, dither, moons, out);
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
    let params = unsafe { slice::from_raw_parts(params_ptr, planet_core::NUM_PARAMS) };
    planet_core::render_rgba_params(size, type_idx as usize, seed, angle, params, out);
}

/// Number of planet types (for the JS "random type" picker).
#[no_mangle]
pub extern "C" fn type_count() -> u32 {
    planet_core::type_count() as u32
}
