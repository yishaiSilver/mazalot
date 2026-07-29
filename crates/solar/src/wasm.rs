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

// The byte-identical `alloc`/`dealloc` C-ABI pair, emitted in-crate.
wasm_abi::alloc_dealloc!();

// `system_new(seed)` / `system_new_params(seed, count)` / `system_free(ptr)`:
// the opaque-handle trio over `System` (System::generate / generate_n), with the
// exact export names preserved.
wasm_abi::opaque_handle!(System, system_new, system_new_params, system_free);

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

/// Set the live eccentricity multiplier (0 = circular orbits, 1 = as generated,
/// higher = exaggerated ellipses). Rescales the system without regenerating it.
#[no_mangle]
pub extern "C" fn system_set_eccentricity(sys: *mut System, scale: f32) {
    let sys = unsafe { &mut *sys };
    sys.set_eccentricity(scale);
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
    let out = unsafe { wasm_abi::out_rgba(buf, w, h) };
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
    sys: *mut System,
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
    let sys = unsafe { &mut *sys };
    let out = unsafe { wasm_abi::out_rgba(buf, w, h) };
    let cam = Camera { x: cam_x, y: cam_y, zoom };
    // Caches the time-independent backdrop; a still camera skips re-rendering it.
    crate::render_system_cached(sys, w, h, &cam, bgx, bgy, t_orbit, t_spin, t_sun, out);
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

/// Freeze every planet's weather: 1 bakes each cloud deck once and reads it back
/// per pixel (the default), 0 evaluates it live. Frozen is ~2x on a planet that
/// fills the view, at the cost of the deck's billowing and its churning storms;
/// the deck still rotates over the surface either way.
#[no_mangle]
pub extern "C" fn system_set_frozen_clouds(sys: *mut System, on: u32) {
    let sys = unsafe { &mut *sys };
    sys.set_frozen_clouds(on != 0);
}

/// The backdrop alone — no bodies. Pair with [`render_bodies_band`] to split a
/// scene across a worker pool.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn render_backdrop(
    sys: *mut System, buf: *mut u8, w: u32, h: u32,
    cam_x: f32, cam_y: f32, zoom: f32, bgx: f32, bgy: f32,
) {
    let sys = unsafe { &mut *sys };
    let out = unsafe { wasm_abi::out_rgba(buf, w, h) };
    crate::render_backdrop(sys, w, h, &Camera { x: cam_x, y: cam_y, zoom }, bgx, bgy, out);
}

/// The bodies for rows `y0..y1`, drawn over a `w × (y1 - y0)` strip that already
/// holds those rows of the backdrop. `w`/`h` are the FULL frame size — that is
/// what places the camera; only the written window narrows.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn render_bodies_band(
    sys: *mut System, buf: *mut u8, w: u32, h: u32,
    cam_x: f32, cam_y: f32, zoom: f32,
    t_orbit: f32, t_spin: f32, t_sun: f32, y0: u32, y1: u32,
) {
    let sys = unsafe { &*sys };
    let hb = y1.min(h).saturating_sub(y0.min(h));
    let out = unsafe { wasm_abi::out_rgba(buf, w, hb) };
    let cam = Camera { x: cam_x, y: cam_y, zoom };
    crate::render_bodies_band(sys, w, h, &cam, t_orbit, t_spin, t_sun, y0, y1, out);
}

// --- WebGL2 path -----------------------------------------------------------
//
// The GPU draws the scene in three passes — one fullscreen backdrop, the orbit
// dots as points, then one quad per body — so what crosses the ABI is a draw
// list, not pixels. `gl_src_*` hands over the GLSL that goes with it, inside the
// module rather than as sibling files, so a shader cannot go stale against the
// wasm it was built with and the single-file artifact keeps working.

/// The GLSL sources, addressed by [`gl_src_ptr`]. A complete fragment shader is
/// the two preludes followed by one body, so the JS concatenates
/// `[0, 1, k]` for `k` in `2..=4`.
const GL_SRC: &[&str] = &[
    noise_core::GL_PRELUDE,          // 0 — #version, precision, the noise kernels
    dither_core::GL_PRELUDE,         // 1 — Bayer + quantization
    background_core::GL_SHADER,      // 2 — the backdrop
    sun_core::GL_SHADER,             // 3 — the star
    planet_core::GL_SHADER,          // 4 — a planet, in its tile framing
];

/// Number of entries [`gl_src_ptr`] addresses.
#[no_mangle]
pub extern "C" fn gl_src_count() -> u32 {
    GL_SRC.len() as u32
}

/// Pointer to GLSL source `i` in wasm memory, with [`gl_src_len`] bytes of UTF-8
/// after it. Out of range yields the empty string.
#[no_mangle]
pub extern "C" fn gl_src_ptr(i: u32) -> *const u8 {
    GL_SRC.get(i as usize).map_or(core::ptr::null(), |s| s.as_ptr())
}

/// Length of GLSL source `i` in bytes.
#[no_mangle]
pub extern "C" fn gl_src_len(i: u32) -> u32 {
    GL_SRC.get(i as usize).map_or(0, |s| s.len() as u32)
}

/// Floats per body record in the [`gl_bodies`] draw list.
#[no_mangle]
pub extern "C" fn gl_body_stride() -> u32 {
    crate::GL_BODY_STRIDE as u32
}

/// Floats of header in front of each body record's uniform block.
#[no_mangle]
pub extern "C" fn gl_body_header() -> u32 {
    crate::GL_BODY_HEADER as u32
}

/// Most bodies a draw list can hold — how big the caller's record buffer must be,
/// in units of [`gl_body_stride`].
#[no_mangle]
pub extern "C" fn gl_max_bodies() -> u32 {
    crate::GL_MAX_BODIES as u32
}

/// Number of floats [`gl_backdrop`] writes.
#[no_mangle]
pub extern "C" fn gl_backdrop_len() -> u32 {
    background_core::GL_UNIFORMS_LEN as u32
}

/// Fill `out` with the backdrop shader's uniforms and return the star-grid salt
/// that goes with them as `u_skySalt`.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn gl_backdrop(
    sys: *const System, out: *mut f32,
    cam_x: f32, cam_y: f32, zoom: f32, bgx: f32, bgy: f32,
) -> i32 {
    let sys = unsafe { &*sys };
    let dst = unsafe { slice::from_raw_parts_mut(out, background_core::GL_UNIFORMS_LEN) };
    crate::gl_backdrop(sys, &Camera { x: cam_x, y: cam_y, zoom }, bgx, bgy, dst)
}

/// Write the dashed orbit paths as screen-space `(x, y)` pairs into `out`
/// (`cap` pairs of capacity); returns how many points were written.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn gl_orbit_points(
    sys: *const System, out: *mut f32, cap: u32,
    w: u32, h: u32, cam_x: f32, cam_y: f32, zoom: f32,
) -> u32 {
    let sys = unsafe { &*sys };
    let dst = unsafe { slice::from_raw_parts_mut(out, (cap * 2) as usize) };
    crate::gl_orbit_points(sys, w, h, &Camera { x: cam_x, y: cam_y, zoom }, dst) as u32
}

/// Write the back-to-front body draw list into `out`
/// (`gl_max_bodies() * gl_body_stride()` floats of capacity); returns the count.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn gl_bodies(
    sys: *const System, out: *mut f32,
    w: u32, h: u32, cam_x: f32, cam_y: f32, zoom: f32,
    t_orbit: f32, t_spin: f32, t_sun: f32,
) -> u32 {
    let sys = unsafe { &*sys };
    let dst = unsafe { slice::from_raw_parts_mut(out, crate::GL_MAX_BODIES * crate::GL_BODY_STRIDE) };
    let cam = Camera { x: cam_x, y: cam_y, zoom };
    crate::gl_bodies(sys, w, h, &cam, t_orbit, t_spin, t_sun, dst) as u32
}

/// The dashed orbit-path dot size in px, so the GPU point sprite matches the
/// square stamp `paint_orbit` writes.
#[no_mangle]
pub extern "C" fn gl_orbit_width(sys: *const System) -> f32 {
    let sys = unsafe { &*sys };
    // `paint_orbit` stamps a square of half-extent `round((width - 1) / 2)`.
    (((sys.orbit_width - 1.0) * 0.5).round()) * 2.0 + 1.0
}

/// The feature mask the GPU path renders with — `F_ALL`, i.e. the live shader.
/// The `F_BAKED_*` bits buy CPU time at the cost of frozen weather, and the GPU
/// evaluates the noise directly, so it has nothing to switch on.
#[no_mangle]
pub extern "C" fn gl_feat() -> u32 {
    planet_core::F_ALL
}
