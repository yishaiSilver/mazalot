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
//!
//! The byte-identical `alloc`/`dealloc` pair and the opaque-handle
//! `new`/`new_params`/`free` trio come from the shared `wasm-abi` macros so this
//! file only carries the belt-specific accessors, view setter, and render body.

use crate::{Belt, Camera};

// `alloc` + `dealloc` — the pixel-buffer allocator pair.
wasm_abi::alloc_dealloc!();

// `belt_new(seed)` / `belt_new_params(seed, count)` / `belt_free(ptr)`.
wasm_abi::opaque_handle!(Belt, belt_new, belt_new_params, belt_free);

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
    let out = unsafe { wasm_abi::out_rgba(buf, w, h) };
    let cam = Camera { x: cam_x, y: cam_y, zoom };
    crate::render_belt(belt, w, h, &cam, t, out);
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
