//! sun-core — the compact procedural star renderer shared by `solar` (its
//! central sun) and `comet` (its star). Both crates previously carried a
//! near-identical copy; the only real differences were the corona reach and an
//! optional level-of-detail path, which are now parameters. Same inputs =>
//! byte-identical output to the old per-crate copies.
//!
//! The full-fidelity star crate (`star`) is a different, richer renderer — this
//! is deliberately the "lite" tier tuned for a body seen at scene scale.

use dither_core::{bayer, quant};
use noise_core::{clamp01, fbm, mix, ramp3, seed_offsets, smoothstep, worley, Rgb};
use scene_core::Tile;

/// One star archetype: a cool→mid→hot temperature ramp plus a corona tint and
/// granulation-cell frequency. Callers keep their own tables of these.
#[derive(Clone, Copy)]
pub struct StarKind {
    pub name: &'static str,
    pub cool: Rgb,
    pub mid: Rgb,
    pub hot: Rgb,
    pub corona: Rgb,
    pub gran: f32, // granulation cell frequency
}

// ---------------------------------------------------------------------------
// One-dimensional fields, sampled once per bake instead of once per pixel
// ---------------------------------------------------------------------------

/// A pseudo-angle for `(x, y)`: `[0, 4)`, monotone in θ, no transcendentals.
/// Walks the diamond `|x| + |y| = 1` rather than the unit circle, so it is not
/// proportional to θ — monotone is all a table index needs.
#[inline]
fn diamond_angle(y: f32, x: f32) -> f32 {
    if y >= 0.0 {
        if x >= 0.0 {
            y / (x + y)
        } else {
            1.0 - x / (y - x)
        }
    } else if x < 0.0 {
        2.0 - y / (-x - y)
    } else {
        3.0 + x / (x - y)
    }
}

/// Inverse of [`diamond_angle`], for filling a table indexed by it.
fn from_diamond(a: f32) -> (f32, f32) {
    let (dx, dy) = if a < 1.0 {
        (1.0 - a, a)
    } else if a < 2.0 {
        (1.0 - a, 2.0 - a)
    } else if a < 3.0 {
        (a - 3.0, 2.0 - a)
    } else {
        (a - 3.0, a - 4.0)
    };
    let m = (dx * dx + dy * dy).sqrt();
    (dx / m, dy / m)
}

/// Linear interpolation into a table over `u ∈ [0, 1]`.
#[inline]
fn sample(tab: &[f32], u: f32) -> f32 {
    let n = tab.len() - 1;
    let f = clamp01(u) * n as f32;
    let i = (f as usize).min(n - 1);
    let frac = f - i as f32;
    tab[i] + (tab[i + 1] - tab[i]) * frac
}

/// The parts of the star's shading that vary along **one** axis, sampled once
/// per bake rather than once per pixel.
///
/// The granulation cells need per-pixel Worley — they are the visible structure.
/// The corona's streamers do not: they depend only on the angle around the limb,
/// its falloff only on the distance past it, and the disc's limb darkening only
/// on `mu`. That is worth splitting out because the halo annulus is ~1.9× the
/// disc's area, so ~65% of a star tile's shaded pixels were running a two-octave
/// fBm and a `powf` for a smooth 1-D function.
struct Shade {
    /// Streamer brightness around the limb, indexed by [`diamond_angle`]. The
    /// final entry repeats the first so the wrap-around seam interpolates.
    flare: Vec<f32>,
    /// Corona falloff, indexed by `edge / corona_reach` over `[0, 1]`.
    fall: Vec<f32>,
    /// Disc limb darkening, indexed by `mu` over `[0, 1]`.
    limb: Vec<f32>,
}

impl Shade {
    fn build(t: f32, rad_px: f32, corona_reach: f32, corona_oct: u32) -> Shade {
        // One entry per px of outer circumference, ×2.2 because `diamond_angle`
        // covers a turn in 4 units at a non-constant rate — dθ/da is twice as
        // steep at the diagonals, so the circumference alone under-samples those.
        let circ = 2.0 * core::f32::consts::PI * (1.0 + corona_reach) * rad_px;
        let n = ((circ * 2.2) as usize).clamp(64, 8192);
        let flare = (0..=n)
            .map(|i| {
                let a = 4.0 * (i % n) as f32 / n as f32;
                let (u, v) = from_diamond(a);
                0.6 + 0.5 * fbm(u * 5.0, v * 5.0, t * 0.6, corona_oct)
            })
            .collect();
        const N: usize = 256;
        let fall = (0..=N)
            .map(|i| {
                let edge = corona_reach * i as f32 / N as f32;
                smoothstep(corona_reach, 0.0, edge).powf(1.6)
            })
            .collect();
        let limb = (0..=N)
            .map(|i| 0.66 + 0.34 * (i as f32 / N as f32).powf(0.45))
            .collect();
        Shade { flare, fall, limb }
    }

    /// Streamer brightness for the unit direction `(u, v)` off the limb.
    #[inline]
    fn flare(&self, u: f32, v: f32) -> f32 {
        sample(&self.flare, diamond_angle(v, u) * 0.25)
    }
}

/// Per-pixel star surface shade. `warp_oct`/`blotch_oct` are the fBm octave
/// counts for the two secondary noise fields — callers pass `(2, 3)` for full
/// detail or `(1, 2)` for the zoomed-in LOD path (worley stays full, since it
/// carries the visible cell structure).
#[allow(clippy::too_many_arguments)]
fn star_surface(
    sk: &StarKind,
    sx: f32,
    sy: f32,
    sz: f32,
    ofs: [f32; 3],
    t: f32,
    mu: f32,
    warp_oct: u32,
    blotch_oct: u32,
    sh: &Shade,
) -> Rgb {
    let f = sk.gran;
    let (px, py, pz) = (sx + ofs[0], sy + ofs[1], sz + ofs[2]);
    // Boil the cell field slowly over time; sample a warped worley for lanes.
    let warp = 0.5 * fbm(px * 1.6 + t * 0.4, py * 1.6, pz * 1.6 - t * 0.3, warp_oct) - 0.25;
    let w = worley(px * f + warp, py * f + warp, pz * f);
    let blotch = fbm(px * 0.9, py * 0.9, pz * 0.9 + t * 0.2, blotch_oct);
    let cool_region = smoothstep(0.46, 0.30, blotch);
    let lane = smoothstep(0.55, 0.82, w);
    let dark = clamp01(cool_region * 0.85 + lane * 0.4);
    let heat = clamp01(1.0 - 0.9 * dark);
    let mut col = ramp3(sk.cool, sk.mid, sk.hot, heat);
    // Gentle limb darkening: dimmer + cooler at the edge for a spherical read.
    let limb = sample(&sh.limb, mu);
    col = mix(mix(col, sk.cool, 0.20 * (1.0 - mu)), col, mu.sqrt());
    [col[0] * limb, col[1] * limb, col[2] * limb]
}

/// Render the star to a tile of diameter ~`2*rad_px` (+corona margin).
///
/// `corona_reach` is the halo radius past the disc, in disc radii (solar uses
/// 0.7, comet 0.85). `lod_enabled` turns on the large-tile detail thinning
/// (solar), which drops the secondary-fBm octaves once a tile exceeds 200px —
/// they modulate below the Bayer-dither floor at that size, so it's free. Comet
/// passes `false` (always full detail).
pub fn render_star_tile(
    sk: &StarKind,
    seed: u32,
    t: f32,
    rad_px: f32,
    corona_reach: f32,
    lod_enabled: bool,
) -> Tile {
    let size = star_tile_size(rad_px, corona_reach);
    let mut tile = Tile::default();
    render_star_tile_into(&mut tile, sk, seed, t, rad_px, corona_reach, lod_enabled, [0, 0, size, size]);
    tile
}

/// The edge length [`render_star_tile`] produces at this radius and corona
/// reach. Needed up front to ask the compositor which part of the tile will be
/// on screen (`scene_core::visible_tile_rect`).
pub fn star_tile_size(rad_px: f32, corona_reach: f32) -> u32 {
    let margin = rad_px * corona_reach + 3.0;
    (((rad_px + margin) * 2.0).ceil() as u32).max(6)
}

/// [`render_star_tile`] into a tile you already own, shading only `clip`
/// (`[x0, y0, x1, y1)`, tile px). Pass `scene_core::visible_tile_rect` and the
/// part of the tile hanging off the viewport is never shaded.
#[allow(clippy::too_many_arguments)]
pub fn render_star_tile_into(
    tile: &mut Tile,
    sk: &StarKind,
    seed: u32,
    t: f32,
    rad_px: f32,
    corona_reach: f32,
    lod_enabled: bool,
    clip: [u32; 4],
) {
    let size = star_tile_size(rad_px, corona_reach);
    tile.ensure(size);
    let c = size as f32 / 2.0;
    let ofs = seed_offsets(seed, 220.0);
    // LOD: on a large (zoomed-in) tile, thin the secondary-fBm octaves.
    let lod = lod_enabled && size > 200;
    let (warp_oct, blotch_oct) = if lod { (1, 2) } else { (2, 3) };
    let corona_oct = if lod { 2 } else { 3 };
    // The three one-dimensional fields, sampled once for the whole tile.
    let sh = Shade::build(t, rad_px, corona_reach, corona_oct);
    let px = &mut tile.px;

    // Past the corona a tile is empty, so a row need only be walked across the
    // disc-plus-halo circle it intersects.
    let reach = 1.0 + corona_reach;
    let [clip_x0, clip_y0, clip_x1, clip_y1] = [
        clip[0].min(size),
        clip[1].min(size),
        clip[2].min(size),
        clip[3].min(size),
    ];
    for iy in clip_y0..clip_y1 {
        let ny = (c - (iy as f32 + 0.5)) / rad_px;
        let half = (reach * reach - ny * ny).max(0.0).sqrt() * rad_px + 1.0;
        let x0 = clip_x0.max((c - half).floor().max(0.0) as u32);
        let x1 = clip_x1.min((c + half).ceil().clamp(0.0, size as f32) as u32);
        let row = (iy * size * 4) as usize;
        let span = |a: u32, b: u32| row + (a * 4) as usize..row + (b * 4) as usize;
        if x1 <= x0 {
            px[span(clip_x0, clip_x1)].fill(0);
            continue;
        }
        px[span(clip_x0, x0)].fill(0);
        px[span(x1, clip_x1)].fill(0);
        for ix in x0..x1 {
            let nx = (ix as f32 + 0.5 - c) / rad_px;
            let d2 = nx * nx + ny * ny;
            let r = d2.sqrt();

            let (mut col, mut a);
            if d2 <= 1.0 {
                let nz = (1.0 - d2).sqrt();
                col = star_surface(sk, nx, ny, nz, ofs, t, nz, warp_oct, blotch_oct, &sh);
                a = 1.0;
            } else {
                col = [0.0, 0.0, 0.0];
                a = 0.0;
            }
            // Corona halo: a soft, shimmering falloff past the limb.
            let edge = r - 1.0;
            if edge > 0.0 && edge < corona_reach {
                // Out here `r > 1`, so the unit direction the streamers want is
                // just `(nx, ny) / r` — no `atan2`/`cos`/`sin` round-trip.
                let inv_r = 1.0 / r;
                let flare = sh.flare(nx * inv_r, ny * inv_r);
                let fall = sample(&sh.fall, edge / corona_reach);
                let glow = clamp01(fall * flare);
                let cc = [sk.corona[0] * glow, sk.corona[1] * glow, sk.corona[2] * glow];
                col = [
                    clamp01(col[0] * a + cc[0]),
                    clamp01(col[1] * a + cc[1]),
                    clamp01(col[2] * a + cc[2]),
                ];
                a = clamp01(a.max(glow));
            }

            let q = quant(col, bayer(ix, iy), 24.0, 0.7);
            let idx = ((iy * size + ix) * 4) as usize;
            px[idx] = (q[0] * 255.0) as u8;
            px[idx + 1] = (q[1] * 255.0) as u8;
            px[idx + 2] = (q[2] * 255.0) as u8;
            px[idx + 3] = (clamp01(a) * 255.0) as u8;
        }
    }
}

// ---------------------------------------------------------------------------

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

    /// Usable as a table index only if it rises monotonically with θ over a
    /// full turn — and the inverse has to round-trip.
    #[test]
    fn diamond_angle_is_monotone_and_invertible() {
        let n = 4000;
        let mut prev = -1.0f32;
        for i in 0..n {
            let th = core::f32::consts::TAU * i as f32 / n as f32;
            let (x, y) = (th.cos(), th.sin());
            let a = diamond_angle(y, x);
            assert!((0.0..4.0).contains(&a), "pseudo-angle {a} out of range at θ={th}");
            assert!(a >= prev - 1e-4, "pseudo-angle fell from {prev} to {a} at θ={th}");
            prev = a;
            // The inverse must land back on the same direction.
            let (u, v) = from_diamond(a);
            assert!(
                (u - x).abs() < 2e-3 && (v - y).abs() < 2e-3,
                "from_diamond({a}) = ({u}, {v}), expected ({x}, {y})"
            );
        }
    }

    /// The tables are an approximation, so pin how close they stay. Too few
    /// entries or a bad index shows up as banding rings or angular stair-steps
    /// in the halo, which no single still frame makes obvious.
    #[test]
    fn tabulated_shading_matches_direct_evaluation() {
        for &rad in &[8.0f32, 24.0, 60.0, 110.0, 176.0] {
            for &reach in &[0.7f32, 0.85] {
                let size = star_tile_size(rad, reach);
                let tile = render_star_tile(&SK, 7, 1.7, rad, reach, true);
                let sh = Shade::build(1.7, rad, reach, if size > 200 { 2 } else { 3 });
                let c = size as f32 / 2.0;
                let (mut worst, mut n) = (0i32, 0usize);
                for iy in 0..size {
                    for ix in 0..size {
                        // Only the halo depends on `flare`/`fall`.
                        let nx = (ix as f32 + 0.5 - c) / rad;
                        let ny = (c - (iy as f32 + 0.5)) / rad;
                        let r = (nx * nx + ny * ny).sqrt();
                        let edge = r - 1.0;
                        if edge <= 0.0 || edge >= reach {
                            continue;
                        }
                        let inv = 1.0 / r;
                        let oct = if size > 200 { 2 } else { 3 };
                        let flare =
                            0.6 + 0.5 * fbm(nx * inv * 5.0, ny * inv * 5.0, 1.7 * 0.6, oct);
                        let fall = smoothstep(reach, 0.0, edge).powf(1.6);
                        let want = clamp01(fall * flare);
                        let got = clamp01(sample(&sh.fall, edge / reach) * sh.flare(nx * inv, ny * inv));
                        let d = ((want - got) * 255.0).abs() as i32;
                        worst = worst.max(d);
                        n += 1;
                        let a = tile.px[((iy * size + ix) * 4 + 3) as usize];
                        assert!(a > 0 || want < 0.02, "halo px ({ix},{iy}) is transparent");
                    }
                }
                assert!(n > 20, "rad {rad}: only {n} halo px sampled");

                // `mu^0.45` has infinite slope at 0, so the very limb is where
                // interpolation is worst.
                let mut lw = 0i32;
                for i in 0..=2048 {
                    let mu = i as f32 / 2048.0;
                    let want = 0.66 + 0.34 * mu.powf(0.45);
                    lw = lw.max(((want - sample(&sh.limb, mu)) * 255.0).abs() as i32);
                }
                assert!(lw <= 2, "limb darkening table is off by {lw}/255");
                // Exact to the byte from rad 24 up. Only a tiny star, whose
                // table hits the 64-entry floor, drifts — by a third of a
                // quantization level, on a halo a few px wide.
                let bound = if rad >= 24.0 { 0 } else { 3 };
                assert!(
                    worst <= bound,
                    "rad {rad}, reach {reach}: tabulated corona is off by {worst}/255 \
                     over {n} halo px (allowed {bound}; one quantization level is 255/24 ≈ 11)"
                );
            }
        }
    }
}
