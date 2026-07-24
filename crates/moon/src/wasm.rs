//! WASM glue for the browser demo. Raw C ABI — no wasm-bindgen.
//!
//! Like `solar`, a scene has a small generated structure (parent + moon list)
//! that's cheap but not free to build, so JS builds it ONCE via [`moon_new`] and
//! passes the opaque pointer back into every [`render`]. The flow:
//!   1. `alloc(len)` -> a pixel buffer in wasm linear memory
//!   2. `moon_new(seed)` -> an opaque `*mut MoonSystem`
//!   3. `render(sys, buf, w, h, cam_x, cam_y, zoom, t)` -> fills RGBA
//!   4. read the bytes from `memory.buffer`, draw to the canvas
//!   5. `moon_free(sys)` / `dealloc(buf, len)` when done

use crate::{Camera, MoonSystem};
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

/// Generate the planet + moons for `seed` and hand back an opaque pointer.
#[no_mangle]
pub extern "C" fn moon_new(seed: u32) -> *mut MoonSystem {
    Box::into_raw(Box::new(MoonSystem::generate(seed)))
}

/// Generate for `seed`, forcing the moon count when `count > 0` (0 = the
/// seed-derived 2..=5). Opaque pointer, freed with [`moon_free`].
#[no_mangle]
pub extern "C" fn moon_new_params(seed: u32, count: u32) -> *mut MoonSystem {
    Box::into_raw(Box::new(MoonSystem::generate_n(seed, count)))
}

/// Free a system previously returned by [`moon_new`].
#[no_mangle]
pub extern "C" fn moon_free(ptr: *mut MoonSystem) {
    if !ptr.is_null() {
        unsafe {
            drop(Box::from_raw(ptr));
        }
    }
}

/// Render the scene into the RGBA buffer at `buf` (must be >= w*h*4 bytes) at
/// time `t`, with the camera showing world `(cam_x, cam_y)` at the viewport
/// centre scaled by `zoom`.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn render(
    sys: *const MoonSystem,
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
    let cam = Camera { x: cam_x, y: cam_y, zoom };
    sys.render(w, h, &cam, t, out);
}

/// Set the dashed orbit-line thickness in pixels (clamped 1..=6).
#[no_mangle]
pub extern "C" fn moon_set_orbit_width(sys: *mut MoonSystem, px: f32) {
    let sys = unsafe { &mut *sys };
    sys.set_orbit_width(px);
}

/// Number of moons in the system.
#[no_mangle]
pub extern "C" fn moon_count(sys: *const MoonSystem) -> u32 {
    let sys = unsafe { &*sys };
    sys.moon_count() as u32
}

/// The archetype index of moon `i` (maps to `moon_kind_name` in JS).
#[no_mangle]
pub extern "C" fn moon_kind_at(sys: *const MoonSystem, i: u32) -> u32 {
    let sys = unsafe { &*sys };
    sys.moons.get(i as usize).map(|m| m.kind as u32).unwrap_or(0)
}

/// The parent-planet archetype index (maps to `parent_kind_name` in JS).
#[no_mangle]
pub extern "C" fn parent_kind_of(sys: *const MoonSystem) -> u32 {
    let sys = unsafe { &*sys };
    sys.parent_kind as u32
}

/// Outermost orbit radius in world units — for an initial zoom-to-fit.
#[no_mangle]
pub extern "C" fn system_extent(sys: *const MoonSystem) -> f32 {
    let sys = unsafe { &*sys };
    sys.extent()
}
