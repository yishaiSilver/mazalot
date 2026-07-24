//! WASM glue for the browser demo. Raw C ABI — no wasm-bindgen.
//!
//! Like `solar`, a galaxy is a small generated structure that JS builds ONCE via
//! [`galaxy_new`] and passes the opaque pointer back into every [`render_map`].
//! Flow:
//!   1. `alloc(len)`      -> a pixel buffer in wasm linear memory
//!   2. `galaxy_new(seed)`-> an opaque `*mut Galaxy`
//!   3. `render_map(gal, buf, w, h, cam_x, cam_y, zoom, t, sel, hover)` -> RGBA
//!   4. read the bytes from `memory.buffer`, draw to the canvas
//!   5. per node, `node_seed(gal, i)` hands the system seed to `solar` to drill in
//!   6. `galaxy_free(gal)` / `dealloc(buf, len)` when done

use crate::{Camera, Galaxy};
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

/// Generate the galaxy for `seed` and hand back an opaque pointer.
#[no_mangle]
pub extern "C" fn galaxy_new(seed: u32) -> *mut Galaxy {
    Box::into_raw(Box::new(Galaxy::generate(seed)))
}

/// Generate a galaxy with structural overrides: `count` systems (0 = seed
/// default), `link_density` in [0,1] (negative = default), `arms` spiral arms
/// (0 = seed default). Opaque pointer, freed with [`galaxy_free`].
#[no_mangle]
pub extern "C" fn galaxy_new_params(seed: u32, count: u32, link_density: f32, arms: u32) -> *mut Galaxy {
    Box::into_raw(Box::new(Galaxy::generate_params(seed, count, link_density, arms)))
}

/// Live view multipliers (glyph size scale, haze intensity) — no regeneration.
#[no_mangle]
pub extern "C" fn galaxy_set_view(gal: *mut Galaxy, node_scale: f32, haze: f32) {
    let gal = unsafe { &mut *gal };
    gal.set_view(node_scale, haze);
}

/// Free a galaxy previously returned by [`galaxy_new`].
#[no_mangle]
pub extern "C" fn galaxy_free(ptr: *mut Galaxy) {
    if !ptr.is_null() {
        unsafe {
            drop(Box::from_raw(ptr));
        }
    }
}

/// Render the galaxy map into the RGBA buffer at `buf` (>= w*h*4 bytes). `t`
/// drives twinkle/pulse; `sel`/`hover` are node indices to highlight (−1 none).
/// Caches the time-independent backdrop; a still camera skips re-rendering it.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn render_map(
    gal: *mut Galaxy,
    buf: *mut u8,
    w: u32,
    h: u32,
    cam_x: f32,
    cam_y: f32,
    zoom: f32,
    t: f32,
    sel: i32,
    hover: i32,
) {
    let gal = unsafe { &mut *gal };
    let out = unsafe { slice::from_raw_parts_mut(buf, (w * h * 4) as usize) };
    let cam = Camera { x: cam_x, y: cam_y, zoom };
    crate::render_map_cached(gal, w, h, &cam, t, sel, hover, out);
}

/// Number of systems in the galaxy.
#[no_mangle]
pub extern "C" fn node_count(gal: *const Galaxy) -> u32 {
    let gal = unsafe { &*gal };
    gal.nodes.len() as u32
}

/// The `system_seed` of node `i` — hand this to solar's `system_new` to drill in.
#[no_mangle]
pub extern "C" fn node_seed(gal: *const Galaxy, i: u32) -> u32 {
    let gal = unsafe { &*gal };
    gal.nodes.get(i as usize).map(|n| n.system_seed).unwrap_or(0)
}

/// Star archetype index of node `i` (== solar sun kind → `SUN_NAMES` in JS).
#[no_mangle]
pub extern "C" fn node_star(gal: *const Galaxy, i: u32) -> u32 {
    let gal = unsafe { &*gal };
    gal.nodes.get(i as usize).map(|n| n.star as u32).unwrap_or(0)
}

/// Region/faction index of node `i` (→ `REGION_NAMES` in JS).
#[no_mangle]
pub extern "C" fn node_region(gal: *const Galaxy, i: u32) -> u32 {
    let gal = unsafe { &*gal };
    gal.nodes.get(i as usize).map(|n| n.region as u32).unwrap_or(0)
}

/// Hyperlane count (graph degree) of node `i`.
#[no_mangle]
pub extern "C" fn node_degree(gal: *const Galaxy, i: u32) -> u32 {
    let gal = unsafe { &*gal };
    gal.nodes.get(i as usize).map(|n| n.degree as u32).unwrap_or(0)
}

/// Write node `i`'s world position into `out` (2 f32: x, y) — for camera focus.
#[no_mangle]
pub extern "C" fn node_pos(gal: *const Galaxy, i: u32, out: *mut f32) {
    let gal = unsafe { &*gal };
    let (x, y) = gal.nodes.get(i as usize).map(|n| (n.x, n.y)).unwrap_or((0.0, 0.0));
    let dst = unsafe { slice::from_raw_parts_mut(out, 2) };
    dst[0] = x;
    dst[1] = y;
}

/// Index of the system nearest world `(wx, wy)` within a screen pick tolerance
/// (needs `zoom` to size the tolerance), or −1. Powers hover + click-to-select.
#[no_mangle]
pub extern "C" fn node_at(gal: *const Galaxy, cam_x: f32, cam_y: f32, zoom: f32, wx: f32, wy: f32) -> i32 {
    let gal = unsafe { &*gal };
    let cam = Camera { x: cam_x, y: cam_y, zoom };
    crate::node_at(gal, &cam, wx, wy)
}

/// Farthest node radius (+margin) for an initial fit-zoom.
#[no_mangle]
pub extern "C" fn galaxy_extent(gal: *const Galaxy) -> f32 {
    let gal = unsafe { &*gal };
    gal.extent()
}

/// Number of faction regions defined (for a JS sanity check against REGION_NAMES).
#[no_mangle]
pub extern "C" fn region_name_count() -> u32 {
    crate::region_count_total() as u32
}

/// The disc-inclination vertical squash applied in the renderer. JS uses it in
/// `toWorld`/pan/fit so cursor picking and panning match the tilted projection.
#[no_mangle]
pub extern "C" fn galaxy_incline() -> f32 {
    crate::INCLINE
}
