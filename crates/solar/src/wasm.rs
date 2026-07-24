//! WASM glue for the browser demo. Raw C ABI — no wasm-bindgen.
//!
//! Unlike `planet`/`star` (one stateless body per call), a system has a small
//! generated structure (star + planet list) that's cheap but not free to build,
//! so JS builds it ONCE via [`system_new`] and passes the opaque pointer back
//! into every [`render`]. The flow:
//!   1. `alloc(len)` -> a pixel buffer in wasm linear memory
//!   2. `system_new(seed)` -> an opaque `*mut System`
//!   3. `render(sys, buf, w, h, cam_x, cam_y, zoom, t)` -> fills RGBA
//!   4. read the bytes from `memory.buffer`, draw to the canvas
//!   5. `system_free(sys)` / `dealloc(buf, len)` when done

use crate::{Camera, System};
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
pub extern "C" fn system_new(seed: u32) -> *mut System {
    Box::into_raw(Box::new(System::generate(seed)))
}

/// Generate a system for `seed`, forcing the planet count when `count > 0`
/// (0 = the seed-derived 4..=8). Opaque pointer, freed with [`system_free`].
#[no_mangle]
pub extern "C" fn system_new_params(seed: u32, count: u32) -> *mut System {
    Box::into_raw(Box::new(System::generate_n(seed, count)))
}

/// Set the live view multipliers (planet spacing, planet/sun size, per-body
/// pixelation) and per-body detail caps (max tile radius, px). These rescale the
/// existing system without regenerating it, so the sliders are smooth and the
/// worlds keep their identity.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn system_set_view(
    sys: *mut System,
    spacing: f32,
    planet_size: f32,
    sun_size: f32,
    planet_pixel: f32,
    sun_pixel: f32,
    planet_detail: f32,
    sun_detail: f32,
    star_density: f32,
    star_parallax: f32,
) {
    let sys = unsafe { &mut *sys };
    sys.set_view(
        spacing, planet_size, sun_size, planet_pixel, sun_pixel, planet_detail, sun_detail,
        star_density, star_parallax,
    );
}

/// Set the dashed orbit-path line thickness in pixels (clamped to 1..=6).
#[no_mangle]
pub extern "C" fn system_set_orbit_width(sys: *mut System, px: f32) {
    let sys = unsafe { &mut *sys };
    sys.set_orbit_width(px);
}

/// Free a system previously returned by [`system_new`].
#[no_mangle]
pub extern "C" fn system_free(ptr: *mut System) {
    if !ptr.is_null() {
        unsafe {
            drop(Box::from_raw(ptr));
        }
    }
}

/// Render the system into the RGBA buffer at `buf` (must be >= w*h*4 bytes) with
/// one clock for everything. Kept for simple callers (e.g. the menu thumbnail).
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn render(
    sys: *const System,
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
    // Static single frame (menu thumbnail): screen-space bg offset = cam·zoom.
    crate::render_system(sys, w, h, &cam, cam_x * zoom, cam_y * zoom, t, t, t, out);
}

/// Render with independent clocks: `t_orbit` (orbital motion), `t_spin` (planet
/// axial spin + weather), `t_sun` (star boil/corona), plus `bgx`/`bgy` — the
/// accumulated SCREEN-space camera pan that drives the background parallax at a
/// zoom-independent rate. The web demo accumulates each at its own rate.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn render_t(
    sys: *const System,
    buf: *mut u8,
    w: u32,
    h: u32,
    cam_x: f32,
    cam_y: f32,
    zoom: f32,
    bgx: f32,
    bgy: f32,
    t_orbit: f32,
    t_spin: f32,
    t_sun: f32,
) {
    let sys = unsafe { &*sys };
    let out = unsafe { slice::from_raw_parts_mut(buf, (w * h * 4) as usize) };
    let cam = Camera { x: cam_x, y: cam_y, zoom };
    crate::render_system(sys, w, h, &cam, bgx, bgy, t_orbit, t_spin, t_sun, out);
}

/// Number of planets in the system.
#[no_mangle]
pub extern "C" fn planet_count(sys: *const System) -> u32 {
    let sys = unsafe { &*sys };
    sys.planets.len() as u32
}

/// The archetype index of planet `i` (maps to `planet_kind_name` in JS).
#[no_mangle]
pub extern "C" fn planet_kind_at(sys: *const System, i: u32) -> u32 {
    let sys = unsafe { &*sys };
    sys.planets.get(i as usize).map(|p| p.kind as u32).unwrap_or(0)
}

/// The star archetype index (maps to `sun_kind_name` in JS).
#[no_mangle]
pub extern "C" fn sun_kind_of(sys: *const System) -> u32 {
    let sys = unsafe { &*sys };
    sys.sun_kind as u32
}

/// Outermost orbit radius in world units — for an initial zoom-to-fit.
#[no_mangle]
pub extern "C" fn system_extent(sys: *const System) -> f32 {
    let sys = unsafe { &*sys };
    sys.extent()
}

/// Write planet `i`'s world position at time `t` into `out` (2 f32: x, y).
/// Lets a JS camera lock onto and follow a body as it orbits.
#[no_mangle]
pub extern "C" fn planet_pos(sys: *const System, i: u32, t: f32, out: *mut f32) {
    let sys = unsafe { &*sys };
    let (x, y) = crate::planet_world_pos(sys, i as usize, t);
    let dst = unsafe { slice::from_raw_parts_mut(out, 2) };
    dst[0] = x;
    dst[1] = y;
}

/// Index of the planet nearest the viewport centre (or -1) — powers the HUD.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn nearest_center(
    sys: *const System,
    w: u32,
    h: u32,
    cam_x: f32,
    cam_y: f32,
    zoom: f32,
    t: f32,
) -> i32 {
    let sys = unsafe { &*sys };
    let cam = Camera { x: cam_x, y: cam_y, zoom };
    crate::planet_nearest_center(sys, w, h, &cam, t)
}
