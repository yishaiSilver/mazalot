//! The backdrop's WebGL2 uniform block — see the module doc-comment in lib.rs
//! for why this is not compiled into the native build.

use super::*;

/// Length of the array [`gl_uniforms`] fills, in `f32`s.
pub const GL_UNIFORMS_LEN: usize = 24;

/// Floats per star/orbit point sprite: `(x, y, r, g, b)`, the colour already
/// scaled by brightness so an additive blend adds exactly what the CPU adds.
pub const GL_POINT_STRIDE: usize = 5;

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

/// The GLSL ES 3.00 fragment shader body. Not a complete program — see
/// [`GL_SOURCES`].
pub const GL_SHADER: &str = include_str!("backdrop.glsl");

/// The three sources a caller concatenates, in order, for a complete backdrop
/// fragment shader.
pub const GL_SOURCES: &[&str] = &[noise_core::GL_PRELUDE, dither_core::GL_PRELUDE, GL_SHADER];

/// Fill `out` (at least [`GL_UNIFORMS_LEN`] long) with everything
/// `backdrop.glsl` needs for one frame — the ground and its nebula. The stars
/// are a separate pass; see [`gl_star_points`].
///
/// Arguments mirror [`paint_backdrop`]. Everything seeded comes from the same
/// helpers `bake_cells` uses, so the GPU clouds are the same clouds.
pub fn gl_uniforms(
    cfg: &Backdrop,
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

}

/// Write every visible star as a point sprite — `(x, y, r, g, b)` per star, the
/// colour already scaled by brightness — and return how many were written.
///
/// This is [`paint_stars`]'s own walk (they share [`visit_stars`]), emitting
/// vertices instead of pixels. Doing it any other way costs dearly: the
/// fragment-shader version had to test nine cells per layer at every pixel, and
/// measured as half the GPU frame on a full-screen backdrop.
///
/// The coordinates are pixel *centres*, so a 1px point sprite lands on the pixel
/// `paint_stars` writes. Drawn with an additive blend, the result is the same
/// `dst + s * col` the CPU computes.
pub fn gl_star_points<F>(
    sky: &Starfield,
    w: u32,
    h: u32,
    bgx: f32,
    bgy: f32,
    hash: F,
    out: &mut [f32],
) -> usize
where
    F: Fn(i32, i32, i32) -> f32,
{
    let cap = out.len() / GL_POINT_STRIDE;
    let mut n = 0;
    visit_stars(sky, w, h, bgx, bgy, hash, |px, py, s, col| {
        if n >= cap {
            return;
        }
        let o = n * GL_POINT_STRIDE;
        out[o] = px as f32 + 0.5;
        out[o + 1] = py as f32 + 0.5;
        out[o + 2] = s * col[0];
        out[o + 3] = s * col[1];
        out[o + 4] = s * col[2];
        n += 1;
    });
    n
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
        ] {
            assert_eq!(def(name), got, "{name}");
        }
        assert!(
            GL_SHADER.contains(&format!("uniform float B[{GL_UNIFORMS_LEN}];")),
            "backdrop.glsl must declare `uniform float B[{GL_UNIFORMS_LEN}]`"
        );
        // The stars are point sprites now, so nothing about them may still be
        // reaching the fragment shader.
        assert!(!GL_SHADER.contains("starsAt"), "the star gather is back in the shader");
    }

    /// The GPU reads the same seeded constants the CPU bakes with, so a system's
    /// clouds are one shape in both. This is the pairing most likely to rot,
    /// since `bake_cells` could grow a term without the shader noticing.
    #[test]
    fn gl_uniforms_carry_the_baked_constants() {
        let neb = CLOUDY.nebula.unwrap();
        let mut u = vec![0.0; GL_UNIFORMS_LEN];
        gl_uniforms(&CLOUDY, 7, 340.0, -120.0, 1.0, 1.0, &mut u);

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

    /// The point sprites must be the very stars `paint_stars` plots — same
    /// pixels, same colours — since they are now the only stars the GPU draws.
    #[test]
    fn star_points_are_the_stars_paint_stars_plots() {
        const W: u32 = 160;
        const H: u32 = 100;
        let sky = Starfield::new(STAR_TEST_LAYERS, STAR_TEST_TINTS);
        let hash = |cx: i32, cy: i32, salt: i32| hash3(cx, cy, salt.wrapping_add(4321));

        let mut pts = vec![0.0f32; 4096 * GL_POINT_STRIDE];
        let n = gl_star_points(&sky, W, H, 37.0, -12.0, hash, &mut pts);
        assert!(n > 20, "only {n} stars — the sweep is not covering the viewport");

        // Paint the same field over black and check each point matches its pixel.
        let mut buf = vec![0u8; (W * H * 4) as usize];
        paint_stars(&mut buf, W, H, &sky, 37.0, -12.0, hash);
        let mut lit = 0;
        for k in 0..n {
            let o = k * GL_POINT_STRIDE;
            let (px, py) = (pts[o] - 0.5, pts[o + 1] - 0.5);
            assert_eq!(px, px.floor(), "point {k} is not on a pixel centre");
            let idx = ((py as u32 * W + px as u32) * 4) as usize;
            for c in 0..3 {
                let want = (clamp01(pts[o + 2 + c]) * 255.0) as u8;
                assert_eq!(buf[idx + c], want, "star {k} channel {c} at ({px}, {py})");
            }
            lit += 1;
        }
        assert_eq!(lit, n);
    }

    const STAR_TEST_LAYERS: &[StarLayer] = &[
        StarLayer { parallax: 0.13, spacing: 6.0, threshold: 0.80, brightness: 0.55, faint: 0.5, salt: 0 },
        StarLayer { parallax: 0.45, spacing: 11.0, threshold: 0.86, brightness: 1.00, faint: 0.5, salt: 2 },
    ];
    const STAR_TEST_TINTS: StarTints =
        &[(0.46, [0.92, 0.95, 1.00]), (0.78, [1.00, 0.96, 0.78]), (1.01, [0.72, 1.00, 0.95])];

    /// Zoomed in past the cutoff the clouds are off, and then nothing about
    /// where they would have been may leak into the uniforms.
    #[test]
    fn a_faded_nebula_is_switched_off() {
        let mut u = vec![0.0; GL_UNIFORMS_LEN];
        gl_uniforms(&CLOUDY, 7, 0.0, 0.0, 1.0, 0.0, &mut u);
        assert_eq!(u[GL_B_SHOW], 0.0);
        assert_eq!(u[GL_B_STRENGTH], 0.0);
        gl_uniforms(&PLAIN, 7, 0.0, 0.0, 1.0, 1.0, &mut u);
        assert_eq!(u[GL_B_SHOW], 0.0, "a scene with no nebula");
        assert_eq!(u[GL_B_DITHER], PLAIN.dither, "...still grounds its dither");
    }
}
