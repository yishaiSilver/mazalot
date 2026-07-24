//! WASM glue for the browser demo. Raw C ABI — no wasm-bindgen.
//!
//! Like `solar`, a system has a small generated structure (star + body list)
//! that is cheap but not free to build, so JS builds it ONCE via [`orbit_new`]
//! and passes the opaque pointer back into every [`render`]. The flow:
//!   1. `alloc(len)` -> a pixel buffer in wasm linear memory
//!   2. `orbit_new(seed)` -> an opaque `*mut OrbitSystem`
//!   3. `render(sys, buf, w, h, cam_x, cam_y, zoom, t)` -> fills RGBA
//!   4. read the bytes from `memory.buffer`, draw to the canvas
//!   5. `orbit_free(sys)` / `dealloc(buf, len)` when done

use crate::{Camera, OrbitSystem};
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

/// Generate the system for `seed` and hand back an opaque pointer.
#[no_mangle]
pub extern "C" fn orbit_new(seed: u32) -> *mut OrbitSystem {
    Box::into_raw(Box::new(OrbitSystem::generate(seed)))
}

/// Generate a system for `seed`, forcing the body count when `count > 0`
/// (0 = the seed-derived 4..=6). Opaque pointer, freed with [`orbit_free`].
#[no_mangle]
pub extern "C" fn orbit_new_params(seed: u32, count: u32) -> *mut OrbitSystem {
    Box::into_raw(Box::new(OrbitSystem::generate_n(seed, count)))
}

/// Free a system previously returned by [`orbit_new`].
#[no_mangle]
pub extern "C" fn orbit_free(ptr: *mut OrbitSystem) {
    if !ptr.is_null() {
        unsafe {
            drop(Box::from_raw(ptr));
        }
    }
}

/// Render the system into the RGBA buffer at `buf` (must be >= w*h*4 bytes).
/// `zoom <= 0` selects the auto zoom-to-fit camera.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn render(
    sys: *const OrbitSystem,
    buf: *mut u8,
    w: u32,
    h: u32,
    cam_x: f32,
    cam_y: f32,
    zoom: f32,
    t: f32,
) {
    let sys = unsafe { &*sys };
    let out = unsafe { slice::from_raw_parts_mut(buf, (w * h * 4) as usize) };
    let cam = if zoom > 0.0 {
        Some(Camera { x: cam_x, y: cam_y, zoom })
    } else {
        None // auto fit
    };
    sys.render(w, h, cam, t, out);
}

/// Number of orbiting bodies.
#[no_mangle]
pub extern "C" fn body_count(sys: *const OrbitSystem) -> u32 {
    let sys = unsafe { &*sys };
    sys.body_count() as u32
}

/// The archetype index of body `i` (maps to `body_kind_name` in JS).
#[no_mangle]
pub extern "C" fn body_kind_at(sys: *const OrbitSystem, i: u32) -> u32 {
    let sys = unsafe { &*sys };
    sys.body_kind(i as usize) as u32
}

/// Eccentricity of body `i` (0..<1) — for a "how elliptical" HUD readout.
#[no_mangle]
pub extern "C" fn body_ecc_at(sys: *const OrbitSystem, i: u32) -> f32 {
    let sys = unsafe { &*sys };
    sys.body_eccentricity(i as usize)
}

/// The star tint index (maps to `sun_kind_name` in JS).
#[no_mangle]
pub extern "C" fn sun_kind_of(sys: *const OrbitSystem) -> u32 {
    let sys = unsafe { &*sys };
    sys.sun_kind as u32
}

/// Outermost extent in world units — for an initial zoom-to-fit.
#[no_mangle]
pub extern "C" fn system_extent(sys: *const OrbitSystem) -> f32 {
    let sys = unsafe { &*sys };
    sys.extent()
}

/// Write body `i`'s world position at time `t` into `out` (2 f32: x, y).
/// Lets a JS camera lock onto and follow a body along its ellipse.
#[no_mangle]
pub extern "C" fn body_pos(sys: *const OrbitSystem, i: u32, t: f32, out: *mut f32) {
    let sys = unsafe { &*sys };
    let (x, y) = sys.body_world_pos(i as usize, t);
    let dst = unsafe { slice::from_raw_parts_mut(out, 2) };
    dst[0] = x;
    dst[1] = y;
}
