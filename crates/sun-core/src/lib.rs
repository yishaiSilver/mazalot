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

/// Per-pixel star surface shade. `warp_oct`/`blotch_oct` are the fBm octave
/// counts for the two secondary noise fields — callers pass `(2, 3)` for full
/// detail or `(1, 2)` for the zoomed-in LOD path (worley stays full, since it
/// carries the visible cell structure).
pub fn star_surface(
    sk: &StarKind,
    sx: f32,
    sy: f32,
    sz: f32,
    ofs: [f32; 3],
    t: f32,
    mu: f32,
    warp_oct: u32,
    blotch_oct: u32,
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
    let limb = 0.66 + 0.34 * mu.powf(0.45);
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
    let margin = rad_px * corona_reach + 3.0;
    let size = (((rad_px + margin) * 2.0).ceil() as u32).max(6);
    let c = size as f32 / 2.0;
    let ofs = seed_offsets(seed, 220.0);
    // LOD: on a large (zoomed-in) tile, thin the secondary-fBm octaves.
    let lod = lod_enabled && size > 200;
    let (warp_oct, blotch_oct) = if lod { (1, 2) } else { (2, 3) };
    let corona_oct = if lod { 2 } else { 3 };
    let mut px = vec![0u8; (size * size * 4) as usize];

    for iy in 0..size {
        for ix in 0..size {
            let nx = (ix as f32 + 0.5 - c) / rad_px;
            let ny = (c - (iy as f32 + 0.5)) / rad_px;
            let d2 = nx * nx + ny * ny;
            let r = d2.sqrt();

            let (mut col, mut a);
            if d2 <= 1.0 {
                let nz = (1.0 - d2).sqrt();
                col = star_surface(sk, nx, ny, nz, ofs, t, nz, warp_oct, blotch_oct);
                a = 1.0;
            } else {
                col = [0.0, 0.0, 0.0];
                a = 0.0;
            }
            // Corona halo: a soft, shimmering falloff past the limb.
            let edge = r - 1.0;
            if edge > 0.0 && edge < corona_reach {
                let theta = ny.atan2(nx);
                let flare = 0.6 + 0.5 * fbm(theta.cos() * 5.0, theta.sin() * 5.0, t * 0.6, corona_oct);
                let fall = smoothstep(corona_reach, 0.0, edge).powf(1.6);
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
    Tile { px, size }
}
