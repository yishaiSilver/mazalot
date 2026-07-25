//! wasm-abi — the raw C-ABI glue shared by every cdylib space crate's wasm.rs.
//!
//! `#[no_mangle]` symbols must be *defined in* each cdylib to be exported, so
//! the byte-identical boilerplate ships as macros that expand in the crate
//! (`alloc_dealloc!`, `opaque_handle!`) plus tiny generic helpers the crate's
//! own `#[no_mangle]` wrappers call. This crate exports no C symbols itself, so
//! pulling it in adds no new wasm exports — the export set stays exactly what
//! the in-crate macro expansions produce.

use std::slice;

/// Box a value and leak it to a raw pointer for JS to hold as an opaque handle.
#[inline]
pub fn boxed<T>(v: T) -> *mut T {
    Box::into_raw(Box::new(v))
}

/// Reclaim a pointer from [`boxed`]. No-op on null.
///
/// # Safety
/// `p` must be a live pointer previously returned by [`boxed`] and not freed.
#[inline]
pub unsafe fn free_boxed<T>(p: *mut T) {
    if !p.is_null() {
        drop(Box::from_raw(p));
    }
}

/// Reconstruct the `w*h*4` RGBA output slice JS handed us at `buf`.
///
/// # Safety
/// `buf` must point at >= `w*h*4` writable bytes for `'a`.
#[inline]
pub unsafe fn out_rgba<'a>(buf: *mut u8, w: u32, h: u32) -> &'a mut [u8] {
    slice::from_raw_parts_mut(buf, (w * h * 4) as usize)
}

/// Emit the byte-identical `alloc`/`dealloc` C-ABI pair in the calling crate.
#[macro_export]
macro_rules! alloc_dealloc {
    () => {
        /// Allocate `len` bytes in wasm memory and hand the pointer to JS.
        #[no_mangle]
        pub extern "C" fn alloc(len: usize) -> *mut u8 {
            let mut v = ::std::vec::Vec::<u8>::with_capacity(len);
            let ptr = v.as_mut_ptr();
            ::std::mem::forget(v);
            ptr
        }

        /// Free a buffer previously returned by `alloc`.
        #[no_mangle]
        pub extern "C" fn dealloc(ptr: *mut u8, len: usize) {
            if !ptr.is_null() {
                unsafe {
                    drop(::std::vec::Vec::from_raw_parts(ptr, len, len));
                }
            }
        }
    };
}

/// Emit the opaque-handle `new` / `new_params` / `free` C-ABI trio for a scene
/// type `$T` that exposes `generate(u32)` and `generate_n(u32, u32)`. The
/// export names are passed explicitly so each crate keeps its exact ABI.
#[macro_export]
macro_rules! opaque_handle {
    ($T:ty, $new:ident, $new_params:ident, $free:ident) => {
        /// Generate for `seed`; opaque pointer, freed with the paired `*_free`.
        #[no_mangle]
        pub extern "C" fn $new(seed: u32) -> *mut $T {
            $crate::boxed(<$T>::generate(seed))
        }

        /// Generate for `seed`, forcing the count when `count > 0`.
        #[no_mangle]
        pub extern "C" fn $new_params(seed: u32, count: u32) -> *mut $T {
            $crate::boxed(<$T>::generate_n(seed, count))
        }

        /// Free a pointer previously returned by the paired `*_new`.
        #[no_mangle]
        pub extern "C" fn $free(ptr: *mut $T) {
            unsafe { $crate::free_boxed(ptr) }
        }
    };
}
