//! The WebGL2 port's uniform block — see the module doc-comment in lib.rs for
//! why this is not compiled into the native build.

use super::*;

// ---------------------------------------------------------------------------
// WebGL2 port: uniforms
// ---------------------------------------------------------------------------
//
// The GPU runs `shader.glsl`, a transliteration of the pixel loop above. What
// keeps that from being a second copy of the *planet* — as opposed to a second
// copy of the shading — is this block: every constant the fragment shader reads
// is computed HERE, by the same code paths `render_frame` uses, and shipped over
// as one flat float array.
//
// So `TYPES`, `Lod`, `seed_offsets`, `vortex_centres`, `moon_ring` and
// `key_light` have exactly one definition, and adding a planet type still means
// editing one row. Only the ~200 lines of ramps and smoothsteps are duplicated,
// and `scripts/verify-gl.mjs` diffs the two renderers to catch drift there.
//
// There is one shader, evaluated live in both languages. Nothing is precomputed
// per body on either side, which is what lets `verify-gl.mjs` compare them pixel
// for pixel rather than approximately.

/// Length of the array [`gl_uniforms`] fills, in `f32`s.
pub const GL_UNIFORMS_LEN: usize = 160;

// Slot indices. These MUST match the `U_*` defines at the top of `shader.glsl`
// — the pairing IS the wire format, and nothing but `verify-gl.mjs` checks it.
// Some are read only by the tests that pin that pairing; they are the wire
// format either way, so they stay named rather than being inlined.
const GL_U_BASE: usize = 0;
#[allow(dead_code)]
const GL_U_RAD: usize = 22;
const GL_U_ATMO: usize = 32;
#[allow(dead_code)]
const GL_U_OFS: usize = 53;
const GL_U_L: usize = 56;
const GL_U_VORTEX: usize = 59;
const GL_U_MOON: usize = 63;
const GL_U_STOPS: usize = 73;
const GL_U_OCT: usize = 101;
const GL_U_PAL_LEN: usize = 131;
const GL_U_PAL: usize = 132;
const GL_U_SPRITE: usize = 147;
const GL_U_TILE_X0: usize = 148;
const GL_U_TILE_INV: usize = 150;

/// The GLSL ES 3.00 fragment shader body, so the browser gets it from the same
/// module that renders the planets rather than from a file that can go stale.
///
/// Not a complete program: it is concatenated after `noise_core::GL_PRELUDE` and
/// `dither_core::GL_PRELUDE`, which carry the `#version` line and the lattice
/// kernels. See [`GL_SOURCES`].
pub const GL_SHADER: &str = include_str!("shader.glsl");

/// The three sources a caller concatenates, in order, to get a complete planet
/// fragment shader. Exposed as a list so the browser never has to know which
/// crates the prelude comes from.
pub const GL_SOURCES: &[&str] = &[noise_core::GL_PRELUDE, dither_core::GL_PRELUDE, GL_SHADER];

/// The octave count of every fBm field in the shader, in the order the GLSL
/// indexes them (see `O_*` in `shader.glsl`). Derived rather than listed, so a
/// change to [`Lod`] reaches the GPU too.
fn gl_octaves(ct: &PType, lod: Lod, feat: u32, out: &mut [f32]) {
    let band_w = lod.oct(1.3, 5);
    let cloudy = lod.oct(2.0, 5);
    let deck = lod.oct(2.8, 4);
    let terr_full = if ct.ridged { 5 } else { 6 };
    let v = [
        lod.oct(4.0, 2),            // O_SPOT_EDGE
        lod.oct(8.0, 4),            // O_SPOT_STREAK
        lod.oct(9.0, 3),            // O_AURORA
        lod.oct(1.2, 5),            // O_CRATER_M
        lod.oct(ct.freq, terr_full),// O_TERR
        band_w,                     // O_BAND_W
        warp_oct(feat, band_w),     // O_BAND_WARP
        lod.oct(4.0, 4),            // O_BAND_FINE
        lod.oct(ct.freq, 6),        // O_EMIS_ROCK
        lod.oct(2.2, 3),            // O_EMIS_FLOW
        cloudy,                     // O_CLOUDY
        warp_oct(feat, cloudy),     // O_CLOUDY_WARP
        deck,                       // O_DECK
        warp_oct(feat, deck),       // O_DECK_WARP
        lod.oct(5.0, 2),            // O_SHIMMER
    ];
    for (i, o) in v.iter().enumerate() {
        out[i] = *o as f32;
    }
}

/// Fill `out` (at least [`GL_UNIFORMS_LEN`] long) with everything `shader.glsl`
/// needs to shade one body: the type row, the seeded offsets and vortex centres,
/// this frame's moons and trig, the colour ramp, the palette, and the `Lod`
/// octave budget.
///
/// Shared by both framings so they cannot disagree about what a planet is.
/// `seed`, `features` and `palette` also go to the shader as their own scalar
/// uniforms — `seed` because a `u32` does not survive an `f32`.
#[allow(clippy::too_many_arguments)]
fn fill(
    ct: &PType,
    seed: u32,
    angle: f32,
    rad: f32,
    light: [f32; 3],
    dither: f32,
    moons: u32,
    features: u32,
    palette: u32,
    out: &mut [f32],
) {
    let ofs = seed_offsets(seed);
    let lod = Lod::for_disc(rad);
    let (sina, cosa) = angle.sin_cos();
    let (cs, cc) = (angle * 2.0).sin_cos();
    let (mn, nmoon) = if moons != 0 { moon_ring(ct, seed, angle) } else { (Default::default(), 0) };
    let vortex = vortex_centres(seed, ofs);

    out[..GL_UNIFORMS_LEN].fill(0.0);
    let s = &mut out[..GL_UNIFORMS_LEN];
    let scalars = [
        ct.base as u32 as f32,
        ct.freq,
        ct.contrast,
        ct.ridged as u32 as f32,
        ct.clouds,
        ct.caps,
        ct.bands,
        ct.turb,
        ct.glow_e0,
        ct.glow_e1,
        ct.rings as u32 as f32,
        ct.ring_inner,
        ct.ring_outer,
        ct.specular,
        ct.shininess,
        ct.spec_albedo,
        ct.spot,
        ct.lightning,
        ct.aurora,
        ct.storm_cells,
        ct.stops.len() as f32,
        (ct.atmo != [0.0; 3]) as u32 as f32,
        rad,
        sina,
        cosa,
        angle,
        cs,
        cc,
        angle.sin() * 0.6,                          // morph
        (angle * 0.6).sin() * 1.6 * ct.storm_cells, // swirl phase
        nmoon as f32,
        dither,
    ];
    s[GL_U_BASE..GL_U_BASE + scalars.len()].copy_from_slice(&scalars);
    debug_assert_eq!(GL_U_BASE + scalars.len(), GL_U_ATMO);

    for (i, c) in [ct.atmo, ct.light, ct.dark, ct.rock, ct.glow_lo, ct.glow_hi, ct.ring_col, ofs]
        .iter()
        .enumerate()
    {
        s[GL_U_ATMO + i * 3..GL_U_ATMO + i * 3 + 3].copy_from_slice(c);
    }
    s[GL_U_L..GL_U_L + 3].copy_from_slice(&light);
    for (k, (vx, vz)) in vortex.iter().enumerate() {
        s[GL_U_VORTEX + k * 2] = *vx;
        s[GL_U_VORTEX + k * 2 + 1] = *vz;
    }
    for (k, m) in mn.iter().enumerate() {
        s[GL_U_MOON + k * 5..GL_U_MOON + k * 5 + 5].copy_from_slice(&[m.0, m.1, m.2, m.3, m.4]);
    }
    for (i, (t, c)) in ct.stops.iter().enumerate() {
        s[GL_U_STOPS + i * 4..GL_U_STOPS + i * 4 + 4].copy_from_slice(&[*t, c[0], c[1], c[2]]);
    }
    gl_octaves(ct, lod, features, &mut s[GL_U_OCT..GL_U_OCT + 15]);
    gl_octaves(ct, lod.capped(NIGHT_OCT), features, &mut s[GL_U_OCT + 15..GL_U_OCT + 30]);
    if let Some(pal) = self::palette(palette) {
        s[GL_U_PAL_LEN] = pal.len() as f32;
        for (i, c) in pal.iter().enumerate() {
            s[GL_U_PAL + i * 3..GL_U_PAL + i * 3 + 3].copy_from_slice(c);
        }
    }
}

/// Uniforms for the **hero** framing — the GPU twin of [`render_rgba_features`].
/// The planet fills a `size` square under the fixed key light. Arguments mirror
/// that function, minus the pixel buffer.
#[allow(clippy::too_many_arguments)]
pub fn gl_uniforms(
    size: u32,
    type_idx: usize,
    seed: u32,
    angle: f32,
    p: &[f32],
    dither: f32,
    moons: u32,
    features: u32,
    palette: u32,
    out: &mut [f32],
) {
    let ct = ct_with_params(type_idx, p);
    // Same expression as `render_ct_band`, so the disc lands where the CPU path
    // puts it — and `Lod` reads the same radius.
    let rad = (size as f32 * 24.0 / 64.0) * ct.radius_scale;
    fill(&ct, seed, angle, rad, key_light(), dither, moons, features, palette, out);
}

/// Uniforms for the **tile** framing — the GPU twin of [`render_tile_into`].
///
/// A scene draws the body as one screen-space quad instead of shading a tile and
/// blitting it, so the placement travels in the uniforms: `x0`/`y0` are
/// `scene_core`'s destination-rect origin and `scale` the blit's magnification.
/// The shader maps a destination pixel back through the same expression `blit`
/// uses, which is what keeps `planet_pixel` and the detail cap meaningful — the
/// body is blocky in exactly the places the CPU compositor makes it blocky.
///
/// Returns the tile edge in px ([`tile_size`]), which the shader needs as
/// `u_size` and the caller needs to size the quad.
#[allow(clippy::too_many_arguments)]
pub fn gl_tile_uniforms(
    type_idx: usize,
    seed: u32,
    angle: f32,
    light: [f32; 3],
    rad_px: f32,
    features: u32,
    x0: i32,
    y0: i32,
    scale: f32,
    out: &mut [f32],
) -> u32 {
    let ct = &TYPES[type_idx % TYPES.len()];
    // `tile_style`: natural palette, the house dither, no orbiting moons — a
    // scene composites its own bodies.
    fill(ct, seed, angle, rad_px, light, 0.7, 0, features, 0, out);
    out[GL_U_SPRITE] = 1.0;
    out[GL_U_TILE_X0] = x0 as f32;
    out[GL_U_TILE_X0 + 1] = y0 as f32;
    out[GL_U_TILE_INV] = 1.0 / scale;
    tile_size(type_idx, rad_px)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Read `#define NAME value` out of the shader source.
    fn def(name: &str) -> usize {
        let want = format!("#define {name} ");
        GL_SHADER
            .lines()
            .find_map(|l| l.trim().strip_prefix(&want))
            .and_then(|v| v.split_whitespace().next())
            .unwrap_or_else(|| panic!("shader.glsl has no `#define {name}`"))
            .parse()
            .unwrap_or_else(|e| panic!("`#define {name}` is not a number: {e}"))
    }

    /// The uniform array is a wire format between two languages, and a slot that
    /// slips by one silently paints a planet with somebody else's colours rather
    /// than failing. Nothing else checks it below the pixels.
    #[test]
    fn glsl_slot_indices_match_the_rust() {
        for (name, got) in [
            ("U_BASE", GL_U_BASE),
            ("U_RAD", GL_U_RAD),
            ("U_ATMO", GL_U_ATMO),
            ("U_OFS", GL_U_OFS),
            ("U_L", GL_U_L),
            ("U_VORTEX", GL_U_VORTEX),
            ("U_MOON", GL_U_MOON),
            ("U_STOPS", GL_U_STOPS),
            ("U_OCT", GL_U_OCT),
            ("U_PAL_LEN", GL_U_PAL_LEN),
            ("U_PAL", GL_U_PAL),
        ] {
            assert_eq!(def(name), got, "{name}");
        }
        // The GLSL declares the array itself; too short and the tail reads as 0,
        // which looks like "this planet has no rings" rather than like a bug.
        assert!(
            GL_SHADER.contains(&format!("uniform float U[{GL_UNIFORMS_LEN}];")),
            "shader.glsl must declare `uniform float U[{GL_UNIFORMS_LEN}]`"
        );
        // The feature bits are re-declared in GLSL because a shader cannot read
        // Rust constants; all five of them reach the GPU.
        // Collapsed, because the declarations are column-aligned in the source.
        let flat: String = GL_SHADER.split_whitespace().collect::<Vec<_>>().join(" ");
        for (name, bit) in [
            ("F_CLOUD_SHADOW", F_CLOUD_SHADOW),
            ("F_ATMO", F_ATMO),
            ("F_RIM", F_RIM),
            ("F_STARFIELD", F_STARFIELD),
            ("F_NIGHT_LOD", F_NIGHT_LOD),
        ] {
            assert!(
                flat.contains(&format!("const uint {name} = {bit}u;")),
                "shader.glsl must declare `const uint {name} = {bit}u;`"
            );
        }
    }

    /// Every slot the shader names has to be inside the array, and the widest
    /// row/palette has to fit in the space after its base.
    #[test]
    fn gl_uniforms_fills_within_bounds() {
        let widest = TYPES.iter().map(|t| t.stops.len()).max().unwrap();
        assert!(GL_U_STOPS + widest * 4 <= GL_U_OCT, "the ramp overruns the octave table");
        assert!(GL_U_OCT + 30 <= GL_U_PAL_LEN, "the octave table overruns the palette");
        assert!(GL_U_PAL + 5 * 3 <= GL_UNIFORMS_LEN, "the palette overruns the array");

        // Fills every slot it claims, for every type, with every palette.
        let mut u = vec![f32::NAN; GL_UNIFORMS_LEN];
        for t in 0..TYPES.len() {
            let p: Vec<f32> = (0..NUM_PARAMS).map(|i| param(t, i as u32)).collect();
            for pal in 0..4 {
                gl_uniforms(96, t, 7, 0.4, &p, 0.7, 1, F_ALL, pal, &mut u);
                assert!(u.iter().all(|v| v.is_finite()), "type {t}, palette {pal}");
            }
        }
    }

    /// The uniforms are the CPU renderer's own setup, so a few values that are
    /// easy to get wrong in transport are worth pinning against it directly.
    #[test]
    fn gl_uniforms_agree_with_the_cpu_setup() {
        let t = type_index("ringed_giant").unwrap();
        let p: Vec<f32> = (0..NUM_PARAMS).map(|i| param(t, i as u32)).collect();
        let mut u = vec![0.0; GL_UNIFORMS_LEN];
        gl_uniforms(64, t, 3, 1.25, &p, 0.7, 1, F_ALL, 0, &mut u);

        let ct = &TYPES[t];
        // The radius `render_ct_band` computes — the whole framing hangs off it.
        assert_eq!(u[GL_U_RAD], (64.0 * 24.0 / 64.0) * ct.radius_scale);
        assert_eq!(&u[GL_U_OFS..GL_U_OFS + 3], &seed_offsets(3));
        assert_eq!(&u[GL_U_L..GL_U_L + 3], &key_light());
        let (mn, n) = moon_ring(ct, 3, 1.25);
        assert_eq!(u[30], n as f32);
        assert_eq!(u[GL_U_MOON], mn[0].0);
        // A ringed giant is `Banded`, so the band octaves must be the live
        // shader's, not the ceiling.
        let lod = Lod::for_disc(u[GL_U_RAD]);
        assert_eq!(u[GL_U_OCT + 5], lod.oct(1.3, 5) as f32);
        assert_eq!(u[GL_U_OCT + 6], Lod::WARP as f32, "F_CHEAP_WARP is in F_ALL");
        assert_eq!(u[GL_U_OCT + 15 + 2], lod.capped(NIGHT_OCT).oct(9.0, 3) as f32);
    }
}

