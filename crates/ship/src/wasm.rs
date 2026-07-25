//! WASM glue for the browser demo. Raw C ABI — no wasm-bindgen.
//!
//! A ship is a *generated structure* (a part list plus its acceleration grid),
//! cheap but not free to build, so — like `solar` and `comet` — JS builds one
//! and passes the opaque pointer back into every [`render`]. Flow:
//!   1. `alloc(len)` -> a pixel buffer in wasm linear memory
//!   2. `ship_new(class, seed)` (or `ship_new_params`) -> `*mut Ship`
//!   3. `render(ship, buf, w, h, zoom, heading, pan_x, pan_y, thrust, dither,
//!      stars, t)` -> fills RGBA
//!   4. read the bytes from `memory.buffer`, draw to the canvas
//!   5. `ship_free(ship)` / `dealloc(buf, len)` when done
//!
//! Strings (class / role / livery names) are `&'static str` living in wasm
//! memory, so JS reads them straight out of `memory.buffer` given a pointer and
//! a length. The per-ship *designation* is built at runtime, so that one is
//! copied into a caller-supplied buffer instead.

use crate::{Ship, View};
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

// ---------------------------------------------------------------------------
// The static tables (classes, roles, liveries)
// ---------------------------------------------------------------------------

/// Number of ship classes in the table.
#[no_mangle]
pub extern "C" fn class_count() -> u32 {
    crate::class_count() as u32
}
/// Pointer to class `i`'s UTF-8 name inside wasm memory.
#[no_mangle]
pub extern "C" fn class_name_ptr(i: u32) -> *const u8 {
    crate::class_name(i as usize).as_ptr()
}
/// Byte length of class `i`'s name.
#[no_mangle]
pub extern "C" fn class_name_len(i: u32) -> u32 {
    crate::class_name(i as usize).len() as u32
}
/// Role index of class `i` — pair with [`role_name_ptr`].
#[no_mangle]
pub extern "C" fn class_role(i: u32) -> u32 {
    crate::class_role(i as usize) as u32
}
/// Nominal length of class `i`, in metres.
#[no_mangle]
pub extern "C" fn class_length_m(i: u32) -> f32 {
    crate::class_length_m(i as usize)
}

/// Number of roles.
#[no_mangle]
pub extern "C" fn role_count() -> u32 {
    crate::role_count() as u32
}
/// Pointer to role `i`'s UTF-8 name inside wasm memory.
#[no_mangle]
pub extern "C" fn role_name_ptr(i: u32) -> *const u8 {
    crate::role_name(i as usize).as_ptr()
}
/// Byte length of role `i`'s name.
#[no_mangle]
pub extern "C" fn role_name_len(i: u32) -> u32 {
    crate::role_name(i as usize).len() as u32
}

/// Number of livery families (slider param 9 indexes these).
#[no_mangle]
pub extern "C" fn livery_count() -> u32 {
    crate::livery_count() as u32
}
/// Pointer to livery `i`'s UTF-8 name inside wasm memory.
#[no_mangle]
pub extern "C" fn livery_name_ptr(i: u32) -> *const u8 {
    crate::livery_name(i as usize).as_ptr()
}
/// Byte length of livery `i`'s name.
#[no_mangle]
pub extern "C" fn livery_name_len(i: u32) -> u32 {
    crate::livery_name(i as usize).len() as u32
}

/// Number of live structural sliders.
#[no_mangle]
pub extern "C" fn num_params() -> u32 {
    crate::NUM_PARAMS as u32
}
/// Class `class_idx`'s default value for slider `which` — the demo snaps the
/// sliders to these whenever you pick a class.
#[no_mangle]
pub extern "C" fn param(class_idx: u32, which: u32) -> f32 {
    crate::param(class_idx as usize, which)
}

// ---------------------------------------------------------------------------
// Ship lifecycle
// ---------------------------------------------------------------------------

/// Roll a ship of `class_idx` from `seed`. Opaque pointer, freed with
/// [`ship_free`].
#[no_mangle]
pub extern "C" fn ship_new(class_idx: u32, seed: u32) -> *mut Ship {
    Box::into_raw(Box::new(Ship::generate(class_idx as usize, seed)))
}

/// Roll a ship with `n` slider overrides read from `p` (see [`num_params`]).
#[no_mangle]
pub extern "C" fn ship_new_params(class_idx: u32, seed: u32, p: *const f32, n: u32) -> *mut Ship {
    let params: &[f32] = if p.is_null() || n == 0 {
        &[]
    } else {
        unsafe { slice::from_raw_parts(p, n as usize) }
    };
    Box::into_raw(Box::new(Ship::generate_params(class_idx as usize, seed, params)))
}

/// Roll a ship AND its class from `seed` — the "surprise me" button.
#[no_mangle]
pub extern "C" fn ship_random(seed: u32) -> *mut Ship {
    Box::into_raw(Box::new(Ship::random(seed)))
}

/// Free a ship previously returned by one of the constructors.
#[no_mangle]
pub extern "C" fn ship_free(ptr: *mut Ship) {
    if !ptr.is_null() {
        unsafe {
            drop(Box::from_raw(ptr));
        }
    }
}

/// Which class this ship was rolled from.
#[no_mangle]
pub extern "C" fn ship_class(s: *const Ship) -> u32 {
    unsafe { (*s).class as u32 }
}
/// This ship's length in metres (the class nominal, jittered).
#[no_mangle]
pub extern "C" fn ship_length_m(s: *const Ship) -> f32 {
    unsafe { (*s).length_m }
}
/// How many parts the hull assembled into — a nice "complexity" readout.
#[no_mangle]
pub extern "C" fn ship_part_count(s: *const Ship) -> u32 {
    unsafe { (*s).part_count() as u32 }
}
/// Width / length of the ship's own bounding box.
#[no_mangle]
pub extern "C" fn ship_aspect(s: *const Ship) -> f32 {
    unsafe { (*s).aspect() }
}

/// Copy this ship's designation (e.g. `BB-417 Iron Vigil`) into `out` as UTF-8.
/// Returns the number of bytes written (0 if it doesn't fit).
#[no_mangle]
pub extern "C" fn ship_designation(s: *const Ship, out: *mut u8, cap: u32) -> u32 {
    let name = unsafe { (*s).designation() };
    let bytes = name.as_bytes();
    if out.is_null() || bytes.len() > cap as usize {
        return 0;
    }
    let dst = unsafe { slice::from_raw_parts_mut(out, bytes.len()) };
    dst.copy_from_slice(bytes);
    bytes.len() as u32
}

// ---------------------------------------------------------------------------
// Framing + render
// ---------------------------------------------------------------------------

/// Zoom that fits the hull into `w`x`h` at `heading`.
#[no_mangle]
pub extern "C" fn ship_fit_zoom(s: *const Ship, w: u32, h: u32, heading: f32) -> f32 {
    unsafe { (*s).fit_zoom(w, h, heading) }
}
/// Zoom that fits the hull at EVERY heading — stable while it turns.
#[no_mangle]
pub extern "C" fn ship_fit_zoom_spin(s: *const Ship, w: u32, h: u32) -> f32 {
    unsafe { (*s).fit_zoom_spin(w, h) }
}
/// Fit leaving `plume` (fraction of the height) clear astern. Writes
/// `[zoom, pan_y]` into `out` (2 f32).
#[no_mangle]
pub extern "C" fn ship_fit_with_plume(s: *const Ship, w: u32, h: u32, plume: f32, out: *mut f32) {
    let (zoom, pan_y) = unsafe { (*s).fit_with_plume(w, h, plume) };
    let dst = unsafe { slice::from_raw_parts_mut(out, 2) };
    dst[0] = zoom;
    dst[1] = pan_y;
}

/// Render the ship into the RGBA buffer at `buf` (must be >= w*h*4 bytes).
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "C" fn render(
    s: *const Ship,
    buf: *mut u8,
    w: u32,
    h: u32,
    zoom: f32,
    heading: f32,
    pan_x: f32,
    pan_y: f32,
    thrust: f32,
    dither: f32,
    stars: f32,
    t: f32,
) {
    let ship = unsafe { &*s };
    let out = unsafe { slice::from_raw_parts_mut(buf, (w * h * 4) as usize) };
    let view = View { zoom, heading, pan_x, pan_y, thrust, dither, stars };
    ship.render(w, h, &view, t, out);
}
