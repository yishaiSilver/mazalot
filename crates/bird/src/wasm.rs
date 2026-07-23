//! WASM glue for the browser demo. Raw C ABI — no wasm-bindgen.
//!
//! All alien generation lives in `alien-core`; this file only exposes a
//! pointer-based interface JavaScript can call:
//!   1. `alloc(len)` -> a buffer pointer in wasm linear memory
//!   2. `render(ptr, size, seed, phase, energy)` -> fills it with RGBA
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
        unsafe { drop(Vec::from_raw_parts(ptr, len, len)); }
    }
}

/// Render one animated creature frame (RGBA) into the buffer at `ptr`.
/// `phase` in [0,1) drives the idle loop; `energy` scales idle motion; `detail`
/// is the supersample factor (1.0 = chunky, 2.0 = finer pixels, …).
#[no_mangle]
pub extern "C" fn render(ptr: *mut u8, size: u32, seed: u32, phase: f32, energy: f32, detail: f32) {
    let out = unsafe { slice::from_raw_parts_mut(ptr, (size * size * 4) as usize) };
    crate::render_rgba(size, seed, phase, energy, detail, out);
}

/// Design-space grid resolution the creature geometry is authored in.
#[no_mangle]
pub extern "C" fn native_grid() -> u32 {
    crate::GRID
}

/// Default detail (supersample) factor the UI should start at.
#[no_mangle]
pub extern "C" fn default_detail() -> f32 {
    crate::DEFAULT_DETAIL
}
