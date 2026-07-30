//! The star tile's WebGL2 uniform block — see the module doc-comment in lib.rs
//! for why this is not compiled into the native build.

use super::*;

/// Length of the array [`gl_uniforms`] fills, in `f32`s.
pub const GL_UNIFORMS_LEN: usize = 32;

// Slot indices. These MUST match the `S_*` defines at the top of `star.glsl` —
// the pairing IS the wire format.
const GL_S_COOL: usize = 0;
const GL_S_MID: usize = 3;
const GL_S_HOT: usize = 6;
const GL_S_CORONA: usize = 9;
const GL_S_GRAN: usize = 12;
const GL_S_OFS: usize = 13;
const GL_S_T: usize = 16;
const GL_S_RAD: usize = 17;
const GL_S_REACH: usize = 18;
const GL_S_WARP_OCT: usize = 19;
const GL_S_TILE_X0: usize = 22;
const GL_S_TILE_INV: usize = 24;

/// The GLSL ES 3.00 fragment shader body. Not a complete program — see
/// [`GL_SOURCES`].
pub const GL_SHADER: &str = include_str!("star.glsl");

/// The three sources a caller concatenates, in order, for a complete star
/// fragment shader.
pub const GL_SOURCES: &[&str] = &[noise_core::GL_PRELUDE, dither_core::GL_PRELUDE, GL_SHADER];

/// Fill `out` (at least [`GL_UNIFORMS_LEN`] long) with everything `star.glsl`
/// needs to draw one star.
///
/// Arguments mirror [`render_star_tile_into`], with the tile's *placement*
/// standing in for its clip: a scene draws the star as one screen-space quad, so
/// `x0`/`y0` are `scene_core`'s destination-rect origin and `scale` the blit's
/// magnification. The rasterizer does the clipping the `clip` rect used to.
///
/// Returns the tile edge in px ([`star_tile_size`]) — the shader needs it as
/// `u_size` and the caller needs it to size the quad.
#[allow(clippy::too_many_arguments)]
pub fn gl_uniforms(
    sk: &StarKind,
    seed: u32,
    t: f32,
    rad_px: f32,
    corona_reach: f32,
    lod_enabled: bool,
    x0: i32,
    y0: i32,
    scale: f32,
    out: &mut [f32],
) -> u32 {
    let size = star_tile_size(rad_px, corona_reach);
    // Same LOD decision as the CPU tile: on a large (zoomed-in) tile the
    // secondary fBm modulates below the dither floor, so its octaves go.
    let lod = lod_enabled && size > 200;
    let (warp_oct, blotch_oct) = if lod { (1, 2) } else { (2, 3) };
    let corona_oct = if lod { 2 } else { 3 };

    out[..GL_UNIFORMS_LEN].fill(0.0);
    let s = &mut out[..GL_UNIFORMS_LEN];
    s[GL_S_COOL..GL_S_COOL + 3].copy_from_slice(&sk.cool);
    s[GL_S_MID..GL_S_MID + 3].copy_from_slice(&sk.mid);
    s[GL_S_HOT..GL_S_HOT + 3].copy_from_slice(&sk.hot);
    s[GL_S_CORONA..GL_S_CORONA + 3].copy_from_slice(&sk.corona);
    s[GL_S_GRAN] = sk.gran;
    s[GL_S_OFS..GL_S_OFS + 3].copy_from_slice(&seed_offsets(seed, 220.0));
    s[GL_S_T] = t;
    s[GL_S_RAD] = rad_px;
    s[GL_S_REACH] = corona_reach;
    s[GL_S_WARP_OCT] = warp_oct as f32;
    s[GL_S_WARP_OCT + 1] = blotch_oct as f32;
    s[GL_S_WARP_OCT + 2] = corona_oct as f32;
    s[GL_S_TILE_X0] = x0 as f32;
    s[GL_S_TILE_X0 + 1] = y0 as f32;
    s[GL_S_TILE_INV] = 1.0 / scale;
    size
}

#[cfg(test)]
mod tests {
    use super::*;

    const SK: StarKind = StarKind {
        name: "yellow dwarf",
        cool: [0.55, 0.20, 0.02],
        mid: [0.99, 0.74, 0.20],
        hot: [1.0, 0.97, 0.82],
        corona: [1.0, 0.82, 0.42],
        gran: 5.5,
    };

    fn def(name: &str) -> usize {
        let want = format!("#define {name} ");
        GL_SHADER
            .lines()
            .find_map(|l| l.trim().strip_prefix(&want))
            .and_then(|v| v.split_whitespace().next())
            .unwrap_or_else(|| panic!("star.glsl has no `#define {name}`"))
            .parse()
            .unwrap_or_else(|e| panic!("`#define {name}` is not a number: {e}"))
    }

    #[test]
    fn glsl_slot_indices_match_the_rust() {
        for (name, got) in [
            ("S_COOL", GL_S_COOL),
            ("S_MID", GL_S_MID),
            ("S_HOT", GL_S_HOT),
            ("S_CORONA", GL_S_CORONA),
            ("S_GRAN", GL_S_GRAN),
            ("S_OFS", GL_S_OFS),
            ("S_T", GL_S_T),
            ("S_RAD", GL_S_RAD),
            ("S_REACH", GL_S_REACH),
            ("S_WARP_OCT", GL_S_WARP_OCT),
            ("S_TILE_X0", GL_S_TILE_X0),
            ("S_TILE_INV", GL_S_TILE_INV),
        ] {
            assert_eq!(def(name), got, "{name}");
        }
        assert!(
            GL_SHADER.contains(&format!("uniform float S[{GL_UNIFORMS_LEN}];")),
            "star.glsl must declare `uniform float S[{GL_UNIFORMS_LEN}]`"
        );
    }

    /// The GPU has to pick the same tile size and the same LOD tier the CPU
    /// does, or a zoomed star changes detail at a different moment in the two
    /// paths — which reads as the star popping when you switch renderer.
    #[test]
    fn gl_uniforms_track_the_cpu_tile() {
        let mut u = vec![0.0; GL_UNIFORMS_LEN];
        for &rad in &[8.0f32, 24.0, 99.0, 100.0, 176.0] {
            for &reach in &[0.7f32, 0.85] {
                let size = gl_uniforms(&SK, 7, 1.7, rad, reach, true, 3, -5, 2.0, &mut u);
                assert_eq!(size, star_tile_size(rad, reach));
                let lod = size > 200;
                assert_eq!(u[GL_S_WARP_OCT], if lod { 1.0 } else { 2.0 }, "rad {rad}");
                assert_eq!(u[GL_S_WARP_OCT + 1], if lod { 2.0 } else { 3.0 });
                assert_eq!(u[GL_S_WARP_OCT + 2], if lod { 2.0 } else { 3.0 });
                assert_eq!(&u[GL_S_OFS..GL_S_OFS + 3], &seed_offsets(7, 220.0));
                assert_eq!(u[GL_S_TILE_INV], 0.5);
                assert!(u.iter().all(|v| v.is_finite()));
            }
        }
        // `lod_enabled == false` (comet) must never thin, however big the tile.
        gl_uniforms(&SK, 7, 1.7, 400.0, 0.85, false, 0, 0, 1.0, &mut u);
        assert_eq!(u[GL_S_WARP_OCT], 2.0);
    }
}
