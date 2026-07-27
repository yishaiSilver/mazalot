//! GPU face of the planet renderer: the WGSL shader source, plus the [`TYPES`]
//! table flattened into the plain `f32` buffer that shader reads.
//!
//! The browser demo runs the sphere shader on the GPU when WebGPU is available,
//! one invocation per pixel, and falls back to the wasm CPU path when it isn't.
//! That means the *algorithm* necessarily exists twice — once in Rust, once in
//! [`WGSL`] — but the *data* does not: [`type_table`] serialises the same 26 rows
//! the CPU path shades from, so adding an archetype stays a one-row edit in
//! `TYPES` and the GPU picks it up with no shader change.
//!
//! Two consequences worth knowing before you touch either side:
//!
//! * **The layout below is a contract.** `planet.wgsl` indexes this buffer by
//!   the same offsets, spelled as `F_*` / `S_*` constants at the top of the file.
//!   Reordering a field here without editing there silently renders garbage —
//!   the buffer is untyped floats, so nothing will complain. [`GPU_STRIDE`] and
//!   the offsets are asserted against the shader text by the tests below.
//! * **GPU output is not byte-identical to the CPU path**, and is not meant to
//!   be. `fma` contraction, `sin`/`pow` precision and `round()` tie-breaking all
//!   differ from the host, so a handful of pixels land on the far side of a
//!   dither threshold. The two agree on *what* they draw, within a level or two
//!   of quantization — see `docs/webgpu.md` for measured deltas. Anything that
//!   needs reproducible bytes (the native GIF bins, `solar`'s tiles) must keep
//!   using the CPU path, which is unchanged.

use crate::{Base, PType, TYPES};

/// The WGSL mirror of the hero framing ([`crate::render_rgba_styled`]).
///
/// Shipped as a string so the wasm module carries its own shader: the browser
/// demo reads it straight out of linear memory rather than fetching a sibling
/// file, which is what keeps `scripts/make-artifact.sh` able to bundle the demo
/// into one self-contained HTML with no network access.
pub const WGSL: &str = include_str!("planet.wgsl");

/// Ramp stops reserved per row. `TERRAN` is the longest at 7; the slack keeps
/// the stride 4-aligned. `test_stops_fit` fails if a new ramp outgrows it.
pub const GPU_MAX_STOPS: usize = 8;

/// `f32`s per planet type in the [`type_table`] buffer. The layout ends at 75;
/// the stride is rounded up so every row starts on a `vec4` boundary.
pub const GPU_STRIDE: usize = 76;

// -- Field offsets. Mirrored by the `F_*` / `S_*` constants in planet.wgsl. ---
// Scalars first, then the vec3 colours, then the ramp. Grouped by kind rather
// than by the order they appear in `PType` so the shader's accessors stay
// legible; the tests below pin every one of these against the shader text.
const F_BASE: usize = 0; // Base discriminant, see `base_code`
const F_FREQ: usize = 1;
const F_CONTRAST: usize = 2;
const F_RIDGED: usize = 3; // 0 / 1
const F_CLOUDS: usize = 4;
const F_CAPS: usize = 5;
const F_BANDS: usize = 6;
const F_TURB: usize = 7;
const F_GLOW_E0: usize = 8;
const F_GLOW_E1: usize = 9;
const F_RINGS: usize = 10; // 0 / 1
const F_RING_INNER: usize = 11;
const F_RING_OUTER: usize = 12;
const F_RADIUS_SCALE: usize = 13;
const F_SPECULAR: usize = 14;
const F_SHININESS: usize = 15;
const F_SPEC_ALBEDO: usize = 16;
const F_SPOT: usize = 17;
const F_LIGHTNING: usize = 18;
const F_AURORA: usize = 19;
const F_STORM_CELLS: usize = 20;
const F_NSTOPS: usize = 21; // ramp stops actually used by this row
const F_ATMO: usize = 22; // vec3
const F_LIGHT: usize = 25; // vec3
const F_DARK: usize = 28; // vec3
const F_ROCK: usize = 31; // vec3
const F_GLOW_LO: usize = 34; // vec3
const F_GLOW_HI: usize = 37; // vec3
const F_RING_COL: usize = 40; // vec3
/// Ramp stops: `GPU_MAX_STOPS` × (threshold, r, g, b), so 4 floats each.
const F_STOPS: usize = 43;

/// The `Base` discriminant as the shader sees it. The numbering is part of the
/// buffer contract — `planet.wgsl` switches on these.
fn base_code(b: Base) -> f32 {
    match b {
        Base::Terrestrial => 0.0,
        Base::Cratered => 1.0,
        Base::Banded => 2.0,
        Base::Emissive => 3.0,
        Base::Cloudy => 4.0,
    }
}

fn write_rgb(row: &mut [f32], at: usize, c: [f32; 3]) {
    row[at] = c[0];
    row[at + 1] = c[1];
    row[at + 2] = c[2];
}

fn write_row(row: &mut [f32], t: &PType) {
    row[F_BASE] = base_code(t.base);
    row[F_FREQ] = t.freq;
    row[F_CONTRAST] = t.contrast;
    row[F_RIDGED] = t.ridged as u32 as f32;
    row[F_CLOUDS] = t.clouds;
    row[F_CAPS] = t.caps;
    row[F_BANDS] = t.bands;
    row[F_TURB] = t.turb;
    row[F_GLOW_E0] = t.glow_e0;
    row[F_GLOW_E1] = t.glow_e1;
    row[F_RINGS] = t.rings as u32 as f32;
    row[F_RING_INNER] = t.ring_inner;
    row[F_RING_OUTER] = t.ring_outer;
    row[F_RADIUS_SCALE] = t.radius_scale;
    row[F_SPECULAR] = t.specular;
    row[F_SHININESS] = t.shininess;
    row[F_SPEC_ALBEDO] = t.spec_albedo;
    row[F_SPOT] = t.spot;
    row[F_LIGHTNING] = t.lightning;
    row[F_AURORA] = t.aurora;
    row[F_STORM_CELLS] = t.storm_cells;
    write_rgb(row, F_ATMO, t.atmo);
    write_rgb(row, F_LIGHT, t.light);
    write_rgb(row, F_DARK, t.dark);
    write_rgb(row, F_ROCK, t.rock);
    write_rgb(row, F_GLOW_LO, t.glow_lo);
    write_rgb(row, F_GLOW_HI, t.glow_hi);
    write_rgb(row, F_RING_COL, t.ring_col);

    // The ramp is variable-length per type; the shader walks `F_NSTOPS` of them
    // and, like `noise_core::ramp`, falls back to the last stop past the end.
    let n = t.stops.len().min(GPU_MAX_STOPS);
    row[F_NSTOPS] = n as f32;
    for (i, (h, c)) in t.stops.iter().take(n).enumerate() {
        let at = F_STOPS + i * 4;
        row[at] = *h;
        write_rgb(row, at + 1, *c);
    }
}

/// The whole [`TYPES`] table as `TYPES.len() * GPU_STRIDE` floats, ready to
/// upload as a storage buffer. Row `i` starts at `i * GPU_STRIDE`.
pub fn type_table() -> Vec<f32> {
    let mut buf = vec![0.0f32; TYPES.len() * GPU_STRIDE];
    for (i, t) in TYPES.iter().enumerate() {
        write_row(&mut buf[i * GPU_STRIDE..(i + 1) * GPU_STRIDE], t);
    }
    buf
}

/// Number of floats [`type_table`] produces, so a caller can size its buffer
/// before asking for the data.
pub fn type_table_len() -> usize {
    TYPES.len() * GPU_STRIDE
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The ramp slot is fixed-size; a longer ramp would be silently truncated
    /// and the type would render with a clipped palette on the GPU only.
    #[test]
    fn stops_fit() {
        for t in TYPES {
            assert!(
                t.stops.len() <= GPU_MAX_STOPS,
                "type {:?} has {} ramp stops, GPU_MAX_STOPS is {}",
                t.name,
                t.stops.len(),
                GPU_MAX_STOPS
            );
        }
    }

    /// Every field has to land inside the row, ramp included.
    #[test]
    fn stride_covers_layout() {
        assert!(F_STOPS + GPU_MAX_STOPS * 4 <= GPU_STRIDE);
        assert_eq!(GPU_STRIDE % 4, 0, "rows should start on a vec4 boundary");
        assert_eq!(type_table().len(), type_table_len());
    }

    /// The offsets above and the `F_*` / `S_*` constants in planet.wgsl are two
    /// halves of one wire format, and nothing at runtime cross-checks them: the
    /// buffer is untyped floats, so a drifted offset renders a wrong-looking
    /// planet rather than failing. Pin them against the shader text.
    #[test]
    fn shader_offsets_match() {
        let want: &[(&str, usize)] = &[
            ("F_BASE", F_BASE),
            ("F_FREQ", F_FREQ),
            ("F_CONTRAST", F_CONTRAST),
            ("F_RIDGED", F_RIDGED),
            ("F_CLOUDS", F_CLOUDS),
            ("F_CAPS", F_CAPS),
            ("F_BANDS", F_BANDS),
            ("F_TURB", F_TURB),
            ("F_GLOW_E0", F_GLOW_E0),
            ("F_GLOW_E1", F_GLOW_E1),
            ("F_RINGS", F_RINGS),
            ("F_RING_INNER", F_RING_INNER),
            ("F_RING_OUTER", F_RING_OUTER),
            ("F_RADIUS_SCALE", F_RADIUS_SCALE),
            ("F_SPECULAR", F_SPECULAR),
            ("F_SHININESS", F_SHININESS),
            ("F_SPEC_ALBEDO", F_SPEC_ALBEDO),
            ("F_SPOT", F_SPOT),
            ("F_LIGHTNING", F_LIGHTNING),
            ("F_AURORA", F_AURORA),
            ("F_STORM_CELLS", F_STORM_CELLS),
            ("F_NSTOPS", F_NSTOPS),
            ("F_ATMO", F_ATMO),
            ("F_LIGHT", F_LIGHT),
            ("F_DARK", F_DARK),
            ("F_ROCK", F_ROCK),
            ("F_GLOW_LO", F_GLOW_LO),
            ("F_GLOW_HI", F_GLOW_HI),
            ("F_RING_COL", F_RING_COL),
            ("F_STOPS", F_STOPS),
            ("STRIDE", GPU_STRIDE),
            ("MAX_STOPS", GPU_MAX_STOPS),
        ];
        for (name, value) in want {
            let decl = format!("const {name}: u32 = {value}u;");
            assert!(
                WGSL.contains(&decl),
                "planet.wgsl is missing `{decl}` — the shader's buffer layout has \
                 drifted from gpu.rs"
            );
        }
    }
}
