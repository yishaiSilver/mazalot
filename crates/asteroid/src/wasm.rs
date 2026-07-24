//! WASM glue for the browser demo. Raw C ABI — no wasm-bindgen.
//!
//! Like `solar`, a belt has a small generated structure (a few hundred rocks)
//! that's cheap but not free to build, so JS builds it ONCE via [`belt_new`] and
//! passes the opaque pointer back into every [`render`]. The flow:
//!   1. `alloc(len)` -> a pixel buffer in wasm linear memory
//!   2. `belt_new(seed)` -> an opaque `*mut Belt`
//!   3. `render(belt, buf, w, h, cam_x, cam_y, zoom, t)` -> fills RGBA
//!   4. read the bytes from `memory.buffer`, draw to the canvas
//!   5. `belt_free(belt)` / `dealloc(buf, len)` when done

use crate::{Belt, Camera};
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

/// Generate the belt for `seed` and hand back an opaque pointer.
#[no_mangle]
pub extern "C" fn belt_new(seed: u32) -> *mut Belt {
    Box::into_raw(Box::new(Belt::generate(seed)))
}

/// Generate a belt for `seed`, forcing the rock count when `count > 0`
/// (0 = the seed-derived ~380..700). Opaque pointer, freed with [`belt_free`].
#[no_mangle]
pub extern "C" fn belt_new_params(seed: u32, count: u32) -> *mut Belt {
    Box::into_raw(Box::new(Belt::generate_n(seed, count)))
}

/// Set the live view multipliers (belt spacing, rock size, star density) and the
/// central-marker toggle. These rescale the existing belt without regenerating
/// it, so the sliders are smooth and the rocks keep their identity.
#[no_mangle]
pub extern "C" fn belt_set_view(
    belt: *mut Belt,
    spacing: f32,
    rock_size: f32,
    star_density: f32,
    show_center: u32,
) {
    let belt = unsafe { &mut *belt };
    belt.set_view(spacing, rock_size, star_density, show_center != 0);
}

/// Free a belt previously returned by [`belt_new`].
#[no_mangle]
pub extern "C" fn belt_free(ptr: *mut Belt) {
    if !ptr.is_null() {
        unsafe {
            drop(Box::from_raw(ptr));
        }
    }
}

/// Render the belt into the RGBA buffer at `buf` (must be >= w*h*4 bytes) at
/// time `t`. `t` drives both the revolution and the big rocks' tumble.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn render(
    belt: *const Belt,
    buf: *mut u8,
    w: u32,
    h: u32,
    cam_x: f32,
    cam_y: f32,
    zoom: f32,
    t: f32,
) {
    let belt = unsafe { &*belt };
    let out = unsafe { slice::from_raw_parts_mut(buf, (w * h * 4) as usize) };
    let cam = Camera { x: cam_x, y: cam_y, zoom };
    crate::render_belt(belt, w, h, &cam, t, out);
}

/// Render ONLY the rocks onto a zeroed buffer (no starfield, no centre marker),
/// for compositing the belt over another scene. Same args as [`render`].
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn render_overlay(
    belt: *const Belt,
    buf: *mut u8,
    w: u32,
    h: u32,
    cam_x: f32,
    cam_y: f32,
    zoom: f32,
    t: f32,
) {
    let belt = unsafe { &*belt };
    let out = unsafe { slice::from_raw_parts_mut(buf, (w * h * 4) as usize) };
    let cam = Camera { x: cam_x, y: cam_y, zoom };
    crate::render_belt_overlay(belt, w, h, &cam, t, out);
}

/// Number of rocks in the belt.
#[no_mangle]
pub extern "C" fn rock_count(belt: *const Belt) -> u32 {
    let belt = unsafe { &*belt };
    belt.rock_count() as u32
}

/// Outermost belt radius in world units — for an initial zoom-to-fit.
#[no_mangle]
pub extern "C" fn belt_extent(belt: *const Belt) -> f32 {
    let belt = unsafe { &*belt };
    belt.extent()
}
