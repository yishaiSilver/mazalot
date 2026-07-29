//! The backdrop's WebGL2 uniform block — see the module doc-comment in lib.rs
//! for why this is not compiled into the native build.

use super::*;

/// Length of the array [`gl_uniforms`] fills, in `f32`s.
pub const GL_UNIFORMS_LEN: usize = 96;

/// Most layers any scene in the workspace runs (solar uses three).
const GL_MAX_LAYERS: usize = 4;
/// Most tint stops the star palette may carry.
const GL_MAX_TINTS: usize = 8;

// Slot indices. These MUST match the `B_*` defines at the top of
// `backdrop.glsl` — the pairing IS the wire format.
const GL_B_BASE: usize = 0;
const GL_B_DITHER: usize = 3;
const GL_B_SHOW: usize = 4;
const GL_B_CELL: usize = 5;
const GL_B_SCROLL_X: usize = 6;
const GL_B_PHASE_X: usize = 8;
const GL_B_STRENGTH: usize = 10;
const GL_B_NEB_DITHER: usize = 11;
const GL_B_NEB_AMT: usize = 12;
const GL_B_ZA: usize = 13;
const GL_B_TINT_A: usize = 15;
const GL_B_TINT_B: usize = 18;
const GL_B_NLAYERS: usize = 21;
const GL_B_NTINTS: usize = 22;
const GL_B_LAYER: usize = 24;
const GL_B_TINTS: usize = 56;

/// The GLSL ES 3.00 fragment shader body. Not a complete program — see
/// [`GL_SOURCES`].
pub const GL_SHADER: &str = include_str!("backdrop.glsl");

/// The three sources a caller concatenates, in order, for a complete backdrop
/// fragment shader.
pub const GL_SOURCES: &[&str] = &[noise_core::GL_PRELUDE, dither_core::GL_PRELUDE, GL_SHADER];

/// Fill `out` (at least [`GL_UNIFORMS_LEN`] long) with everything
/// `backdrop.glsl` needs for one frame.
///
/// Arguments mirror [`paint_backdrop`] and [`paint_stars`] taken together, which
/// is what the one shader does. `sky_salt` is the third hash axis the scene mixes
/// its seed into — the GPU cannot take a closure, so the scene passes the salt
/// its `paint_stars` callback would have applied, and the shader uses solar's
/// `salt + 17 + layer` convention.
///
/// Everything seeded comes from the same helpers `bake_cells` uses, so the GPU
/// clouds are the same clouds.
#[allow(clippy::too_many_arguments)]
pub fn gl_uniforms(
    cfg: &Backdrop,
    sky: &Starfield,
    seed: u32,
    bgx: f32,
    bgy: f32,
    pan_scale: f32,
    neb_amt: f32,
    out: &mut [f32],
) {
    let si = seed as i32;
    out[..GL_UNIFORMS_LEN].fill(0.0);
    let s = &mut out[..GL_UNIFORMS_LEN];

    s[GL_B_BASE..GL_B_BASE + 3].copy_from_slice(&cfg.base);
    s[GL_B_DITHER] = cfg.dither;

    // Zoomed in far enough and the clouds are gone — the same cutoff
    // `paint_backdrop` uses to skip baking them at all.
    let show = cfg.nebula.is_some() && neb_amt > 0.02;
    s[GL_B_SHOW] = show as u32 as f32;
    if let Some(neb) = cfg.nebula.filter(|_| show) {
        let (ta, tb, za, zb) = nebula_seed(&neb, si);
        let (sx, sy) = cloud_scroll(&neb, si, bgx, bgy, pan_scale);
        s[GL_B_CELL] = neb.cell as f32;
        s[GL_B_SCROLL_X] = sx as f32;
        s[GL_B_SCROLL_X + 1] = sy as f32;
        // The dither rides along with the clouds, mod its 8-px period. Reduced
        // here because GLSL's `&7` would be wrong on a negative index.
        s[GL_B_PHASE_X] = sx.rem_euclid(8) as f32;
        s[GL_B_PHASE_X + 1] = sy.rem_euclid(8) as f32;
        s[GL_B_STRENGTH] = neb.strength;
        s[GL_B_NEB_DITHER] = neb.dither;
        s[GL_B_NEB_AMT] = neb_amt;
        s[GL_B_ZA] = za;
        s[GL_B_ZA + 1] = zb;
        s[GL_B_TINT_A..GL_B_TINT_A + 3].copy_from_slice(&ta);
        s[GL_B_TINT_B..GL_B_TINT_B + 3].copy_from_slice(&tb);
    }

    let d = sky.density.max(0.0);
    let n = sky.layers.len().min(GL_MAX_LAYERS);
    s[GL_B_NLAYERS] = n as f32;
    for (i, layer) in sky.layers.iter().take(n).enumerate() {
        let p = layer.parallax * sky.pan_scale;
        let b = GL_B_LAYER + i * 8;
        s[b..b + 8].copy_from_slice(&[
            bgx * p,
            bgy * p,
            layer.spacing,
            1.0 - (1.0 - layer.threshold) * d,
            layer.brightness,
            layer.faint,
            // The far layer fades (and is skipped) when zoomed in on a body.
            if i == 0 { sky.far_fade } else { 1.0 },
            layer.salt as f32,
        ]);
    }

    let nt = sky.tints.len().min(GL_MAX_TINTS);
    s[GL_B_NTINTS] = nt as f32;
    for (i, (cut, col)) in sky.tints.iter().take(nt).enumerate() {
        s[GL_B_TINTS + i * 4..GL_B_TINTS + i * 4 + 4].copy_from_slice(&[*cut, col[0], col[1], col[2]]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{CLOUDY, PLAIN};

    fn def(name: &str) -> usize {
        let want = format!("#define {name} ");
        GL_SHADER
            .lines()
            .find_map(|l| l.trim().strip_prefix(&want))
            .and_then(|v| v.split_whitespace().next())
            .unwrap_or_else(|| panic!("backdrop.glsl has no `#define {name}`"))
            .parse()
            .unwrap_or_else(|e| panic!("`#define {name}` is not a number: {e}"))
    }

    /// The uniform array is a wire format between two languages; a slot off by
    /// one paints the sky in the nebula's colours rather than failing.
    #[test]
    fn glsl_slot_indices_match_the_rust() {
        for (name, got) in [
            ("B_BASE", GL_B_BASE),
            ("B_DITHER", GL_B_DITHER),
            ("B_SHOW", GL_B_SHOW),
            ("B_CELL", GL_B_CELL),
            ("B_SCROLL_X", GL_B_SCROLL_X),
            ("B_PHASE_X", GL_B_PHASE_X),
            ("B_STRENGTH", GL_B_STRENGTH),
            ("B_NEB_DITHER", GL_B_NEB_DITHER),
            ("B_NEB_AMT", GL_B_NEB_AMT),
            ("B_ZA", GL_B_ZA),
            ("B_TINT_A", GL_B_TINT_A),
            ("B_TINT_B", GL_B_TINT_B),
            ("B_NLAYERS", GL_B_NLAYERS),
            ("B_NTINTS", GL_B_NTINTS),
            ("B_LAYER", GL_B_LAYER),
            ("B_TINTS", GL_B_TINTS),
        ] {
            assert_eq!(def(name), got, "{name}");
        }
        assert!(
            GL_SHADER.contains(&format!("uniform float B[{GL_UNIFORMS_LEN}];")),
            "backdrop.glsl must declare `uniform float B[{GL_UNIFORMS_LEN}]`"
        );
        // The two tables have to fit in the space after their base, and the
        // shader's own loop bounds have to agree with the Rust's caps.
        assert!(GL_B_LAYER + GL_MAX_LAYERS * 8 <= GL_B_TINTS, "the layer table overruns the tints");
        assert!(GL_B_TINTS + GL_MAX_TINTS * 4 <= GL_UNIFORMS_LEN, "the tint table overruns the array");
        assert!(GL_SHADER.contains(&format!("li < {GL_MAX_LAYERS}")), "layer loop bound");
        assert!(GL_SHADER.contains(&format!("i < {GL_MAX_TINTS}")), "tint loop bound");
    }

    /// The GPU reads the same seeded constants the CPU bakes with, so a system's
    /// clouds are one shape in both. This is the pairing most likely to rot,
    /// since `bake_cells` could grow a term without the shader noticing.
    #[test]
    fn gl_uniforms_carry_the_baked_constants() {
        let neb = CLOUDY.nebula.unwrap();
        let sky = Starfield::new(
            &[StarLayer { parallax: 0.2, spacing: 7.0, threshold: 0.8, brightness: 1.0, faint: 0.5, salt: 1 }],
            &[(1.01, [1.0, 1.0, 1.0])],
        );
        let mut u = vec![0.0; GL_UNIFORMS_LEN];
        gl_uniforms(&CLOUDY, &sky, 7, 340.0, -120.0, 1.0, 1.0, &mut u);

        let (ta, tb, za, zb) = nebula_seed(&neb, 7);
        let (sx, sy) = cloud_scroll(&neb, 7, 340.0, -120.0, 1.0);
        assert_eq!(&u[GL_B_TINT_A..GL_B_TINT_A + 3], &ta);
        assert_eq!(&u[GL_B_TINT_B..GL_B_TINT_B + 3], &tb);
        assert_eq!((u[GL_B_ZA], u[GL_B_ZA + 1]), (za, zb));
        assert_eq!((u[GL_B_SCROLL_X], u[GL_B_SCROLL_X + 1]), (sx as f32, sy as f32));
        // Non-negative, because GLSL indexes the Bayer matrix with a mask.
        assert!((0.0..8.0).contains(&u[GL_B_PHASE_X]) && (0.0..8.0).contains(&u[GL_B_PHASE_X + 1]));
        assert!(u.iter().all(|v| v.is_finite()));
    }

    /// Zoomed in past the cutoff the clouds are off, and then nothing about
    /// where they would have been may leak into the uniforms.
    #[test]
    fn a_faded_nebula_is_switched_off() {
        let sky = Starfield::new(&[], &[(1.01, [1.0; 3])]);
        let mut u = vec![0.0; GL_UNIFORMS_LEN];
        gl_uniforms(&CLOUDY, &sky, 7, 0.0, 0.0, 1.0, 0.0, &mut u);
        assert_eq!(u[GL_B_SHOW], 0.0);
        assert_eq!(u[GL_B_STRENGTH], 0.0);
        gl_uniforms(&PLAIN, &sky, 7, 0.0, 0.0, 1.0, 1.0, &mut u);
        assert_eq!(u[GL_B_SHOW], 0.0, "a scene with no nebula");
        assert_eq!(u[GL_B_DITHER], PLAIN.dither, "...still grounds its dither");
    }
}
