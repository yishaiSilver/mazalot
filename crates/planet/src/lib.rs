//! planet — the CLI + browser face of the full-fidelity planet renderer.
//!
//! All of the generation lives in the dependency-free [`planet_core`] rlib, which
//! this crate re-exports wholesale: `TYPES`, `render_rgba` and friends are the
//! same items, reachable at the same paths the bins and `wasm.rs` always used.
//! What lives *here* is the packaging — the native GIF/PNG bins under `src/bin`
//! and the raw C ABI the web demo calls.
//!
//! The split exists because `solar` renders the very same planets as sprite tiles
//! in its system view (see [`planet_core::render_tile`]). Sharing the shader
//! through a plain rlib keeps one copy of it without either cdylib inheriting the
//! other's `#[no_mangle]` exports.

pub use planet_core::*;

// Browser (wasm) C-ABI glue — excluded from native builds. See wasm.rs.
#[cfg(target_arch = "wasm32")]
mod wasm;
