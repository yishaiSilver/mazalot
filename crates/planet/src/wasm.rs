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

/// Number of planet types (for the JS "random type" picker).
#[no_mangle]
pub extern "C" fn type_count() -> u32 {
    crate::type_count() as u32
}

// ---------------------------------------------------------------------------
// WebGPU path. The demo prefers the GPU shader and falls back to `render_styled`
// above when WebGPU is missing, so both of these are optional to the caller.
//
// The shader source and the type table ride along inside the wasm rather than
// sitting next to it as files: `scripts/make-artifact.sh` bundles the demo into
// one self-contained HTML and rejects any surviving `fetch()`, so anything the
// page needs at runtime has to be reachable through linear memory.
// ---------------------------------------------------------------------------

/// Pointer to the WGSL shader source in linear memory (UTF-8, not NUL-terminated
/// — pair it with [`wgsl_len`]).
#[no_mangle]
pub extern "C" fn wgsl_ptr() -> *const u8 {
    planet_core::gpu::WGSL.as_ptr()
}

/// Length of the WGSL source in bytes.
#[no_mangle]
pub extern "C" fn wgsl_len() -> u32 {
    planet_core::gpu::WGSL.len() as u32
}

/// Number of `f32`s [`gpu_table_fill`] will write — allocate that many times 4
/// bytes before calling it.
#[no_mangle]
pub extern "C" fn gpu_table_len() -> u32 {
    planet_core::gpu::type_table_len() as u32
}

/// Write the flattened `TYPES` table for the shader's storage buffer into
/// `ptr`, which must have room for [`gpu_table_len`] `f32`s.
#[no_mangle]
pub extern "C" fn gpu_table_fill(ptr: *mut f32) {
    let table = planet_core::gpu::type_table();
    let out = unsafe { slice::from_raw_parts_mut(ptr, table.len()) };
    out.copy_from_slice(&table);
}

/// Floats per row in the table [`gpu_table_fill`] writes, so the shader and the
/// JS agree on where row `i` starts without hard-coding the stride twice.
#[no_mangle]
pub extern "C" fn gpu_table_stride() -> u32 {
    planet_core::gpu::GPU_STRIDE as u32
}
