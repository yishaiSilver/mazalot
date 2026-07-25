//! WASM glue for the browser demo. Raw C ABI — no wasm-bindgen.
//!
//! Like `solar`, a comet scene has a small generated structure (star + comet
//! list) that's cheap but not free to build, so JS builds it ONCE via
//! [`comet_new`] and passes the opaque pointer back into every [`render`]. Flow:
//!   1. `alloc(len)` -> a pixel buffer in wasm linear memory
//!   2. `comet_new(seed)` -> an opaque `*mut CometScene`
//!   3. `render(scene, buf, w, h, cam_x, cam_y, zoom, t)` -> fills RGBA
//!   4. read the bytes from `memory.buffer`, draw to the canvas
//!   5. `comet_free(scene)` / `dealloc(buf, len)` when done

use crate::{Camera, CometScene};
use std::slice;

// `alloc` / `dealloc` — byte-identical to the hand-rolled pair, emitted in-crate.
wasm_abi::alloc_dealloc!();

// `comet_new(seed)` / `comet_new_params(seed, count)` / `comet_free(ptr)` — the
// opaque-handle trio over `CometScene::generate` / `CometScene::generate_n`.
wasm_abi::opaque_handle!(CometScene, comet_new, comet_new_params, comet_free);

/// Render the scene into the RGBA buffer at `buf` (must be >= w*h*4 bytes) at
/// time `t`, from a camera at `(cam_x, cam_y)` with `zoom`.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn render(
    scene: *const CometScene,
    buf: *mut u8,
    w: u32,
    h: u32,
    cam_x: f32,
    cam_y: f32,
    zoom: f32,
    t: f32,
) {
    let scene = unsafe { &*scene };
    let out = unsafe { wasm_abi::out_rgba(buf, w, h) };
    let cam = Camera { x: cam_x, y: cam_y, zoom };
    scene.render(w, h, &cam, t, out);
}

/// Number of comets in the scene.
#[no_mangle]
pub extern "C" fn comet_count(scene: *const CometScene) -> u32 {
    let scene = unsafe { &*scene };
    scene.comets.len() as u32
}

/// The star archetype index (maps to `star_kind_name` in JS).
#[no_mangle]
pub extern "C" fn star_kind_of(scene: *const CometScene) -> u32 {
    let scene = unsafe { &*scene };
    scene.star_kind as u32
}

/// Outermost aphelion in world units — for an initial zoom-to-fit.
#[no_mangle]
pub extern "C" fn scene_extent(scene: *const CometScene) -> f32 {
    let scene = unsafe { &*scene };
    scene.extent()
}

/// Set the dashed orbit ellipse's stroke width in pixels (clamped 1..=6).
#[no_mangle]
pub extern "C" fn comet_set_orbit_width(scene: *mut CometScene, px: f32) {
    let scene = unsafe { &mut *scene };
    scene.set_orbit_width(px);
}

/// Write comet `i`'s world position at time `t` into `out` (2 f32: x, y).
/// Lets a JS camera lock onto and follow the head as it sweeps its orbit.
#[no_mangle]
pub extern "C" fn comet_pos(scene: *const CometScene, i: u32, t: f32, out: *mut f32) {
    let scene = unsafe { &*scene };
    let (x, y) = crate::comet_world_pos(scene, i as usize, t);
    let dst = unsafe { slice::from_raw_parts_mut(out, 2) };
    dst[0] = x;
    dst[1] = y;
}
