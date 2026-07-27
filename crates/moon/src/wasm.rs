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

// The byte-identical `alloc`/`dealloc` pair and the opaque-handle
// `moon_new` / `moon_new_params` / `moon_free` trio, emitted in-crate by the
// shared wasm-abi macros so the C export names and bytes are exactly what the
// hand-written glue produced. wasm-abi itself exports no C symbols, so the
// crate's wasm export set is unchanged.
wasm_abi::alloc_dealloc!();
wasm_abi::opaque_handle!(MoonSystem, moon_new, moon_new_params, moon_free);

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
    let out = unsafe { wasm_abi::out_rgba(buf, w, h) };
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

/// Freeze the parent planet's weather: 1 bakes the cloud deck once and reads it
/// back per pixel (the default — the parent fills the view, so its shader is the
/// frame), 0 evaluates it live, which billows and churns but costs ~2x.
#[no_mangle]
pub extern "C" fn moon_set_frozen_clouds(sys: *mut MoonSystem, on: u32) {
    let sys = unsafe { &mut *sys };
    sys.frozen_clouds = on != 0;
}
