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

/// Generate the scene for `seed` and hand back an opaque pointer.
#[no_mangle]
pub extern "C" fn comet_new(seed: u32) -> *mut CometScene {
    Box::into_raw(Box::new(CometScene::generate(seed)))
}

/// Generate a scene for `seed`, forcing the comet count when `count > 0`
/// (0 = the seed-derived 1..=3). Opaque pointer, freed with [`comet_free`].
#[no_mangle]
pub extern "C" fn comet_new_params(seed: u32, count: u32) -> *mut CometScene {
    Box::into_raw(Box::new(CometScene::generate_n(seed, count)))
}

/// Free a scene previously returned by [`comet_new`].
#[no_mangle]
pub extern "C" fn comet_free(ptr: *mut CometScene) {
    if !ptr.is_null() {
        unsafe {
            drop(Box::from_raw(ptr));
        }
    }
}

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
    let out = unsafe { slice::from_raw_parts_mut(buf, (w * h * 4) as usize) };
    let cam = Camera { x: cam_x, y: cam_y, zoom };
    scene.render(w, h, &cam, t, out);
}

/// Render ONLY the comets (orbit paths, tails, coma, nuclei) onto a zeroed
/// buffer — no background, no star — for compositing over another scene. Same
/// args as [`render`].
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn render_overlay(
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
    let out = unsafe { slice::from_raw_parts_mut(buf, (w * h * 4) as usize) };
    let cam = Camera { x: cam_x, y: cam_y, zoom };
    scene.render_overlay(w, h, &cam, t, out);
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
