//! WASM glue for a browser demo. Raw C ABI — no wasm-bindgen.
//!
//! All star generation lives in the crate lib; this file only exposes a
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

/// Render a star frame (RGBA) into the buffer at `ptr`.
#[no_mangle]
pub extern "C" fn render(ptr: *mut u8, size: u32, type_idx: u32, seed: u32, angle: f32) {
    let out = unsafe { slice::from_raw_parts_mut(ptr, (size * size * 4) as usize) };
    crate::render_rgba(size, type_idx as usize, seed, angle, out);
}

/// Render with slider params + global style. `params_ptr` points at
/// `num_params()` f32 values in wasm memory (written by JS each frame); `dither`
/// is 0..1; `spin` is whole turns per 2π (0 = boil in place).
#[no_mangle]
pub extern "C" fn render_params(
    ptr: *mut u8,
    size: u32,
    type_idx: u32,
    seed: u32,
    angle: f32,
    params_ptr: *const f32,
    dither: f32,
    spin: f32,
) {
    let out = unsafe { slice::from_raw_parts_mut(ptr, (size * size * 4) as usize) };
    let params = unsafe { slice::from_raw_parts(params_ptr, crate::NUM_PARAMS) };
    crate::render_rgba_params(size, type_idx as usize, seed, angle, params, dither, spin, out);
}

/// Render a chosen look pattern (index into `StarPattern::ALL`: 0 realistic,
/// 1 pixelbands, 2 plasma, 3 celtoon, 4 sunburst) with slider params + style.
#[no_mangle]
pub extern "C" fn render_pattern_params(
    ptr: *mut u8,
    size: u32,
    type_idx: u32,
    seed: u32,
    angle: f32,
    pattern: u32,
    params_ptr: *const f32,
    dither: f32,
    spin: f32,
) {
    let out = unsafe { slice::from_raw_parts_mut(ptr, (size * size * 4) as usize) };
    let params = unsafe { slice::from_raw_parts(params_ptr, crate::NUM_PARAMS) };
    let pat = crate::pattern_from_index(pattern as usize);
    crate::render_rgba_pattern_params(size, type_idx as usize, seed, angle, pat, params, dither, spin, out);
}

/// Number of look patterns (length of `StarPattern::ALL`).
#[no_mangle]
pub extern "C" fn pattern_count() -> u32 {
    crate::pattern_count() as u32
}

/// Read a type's default value for parameter `which` (see `crate::param`), so the
/// sliders can snap to sensible per-type starting values.
#[no_mangle]
pub extern "C" fn param(type_idx: u32, which: u32) -> f32 {
    crate::param(type_idx as usize, which)
}

/// Number of tunable parameters (length of the array `render_params` expects).
#[no_mangle]
pub extern "C" fn num_params() -> u32 {
    crate::NUM_PARAMS as u32
}

/// Number of star types (for the JS "random type" picker).
#[no_mangle]
pub extern "C" fn type_count() -> u32 {
    crate::type_count() as u32
}
