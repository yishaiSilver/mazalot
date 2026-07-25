//! body-core — the compact procedural planet-body renderer shared by `solar`
//! (its draggable planets, including ring worlds) and `moon` (its central
//! parent world). Both crates previously carried a near-identical copy; the
//! differences (rings, emissive self-illumination, the per-body light
//! direction) all ride on struct fields or the `light` argument, so calling
//! this with each crate's own data reproduces its old output byte-for-byte.
//!
//! The full-fidelity `planet` crate is a richer, separate renderer — this is
//! the "lite" tier tuned for a body seen at scene scale. moon's small-moon
//! shader stays moon-local (it is a genuinely different surface).

use dither_core::{bayer, quant};
use noise_core::{clamp01, contrast, fbm, lerp, mix, ramp, seed_offsets, smoothstep, worley, Rgb};
use scene_core::Tile;

pub const KIND_TERRA: u8 = 0; // fbm continents + optional sea/caps
pub const KIND_GAS: u8 = 1; // latitude bands
pub const KIND_EMISSIVE: u8 = 2; // dark rock threaded with lava/glow
pub const KIND_ICE: u8 = 3; // ridged frozen crust

/// One planet archetype: a palette + thresholds + flags. A superset shape —
/// terrestrial worlds ignore the gas/emissive/ring fields and vice versa.
/// Callers keep their own tables of these (use [`base`] to fill the fields a
/// given world doesn't care about).
#[derive(Clone, Copy)]
pub struct BodyKind {
    pub name: &'static str,
    pub kind: u8,
    pub freq: f32,
    pub contr: f32,
    pub stops: &'static [(f32, Rgb)], // terrestrial / ice ramp
    pub band_lo: Rgb,                 // gas
    pub band_hi: Rgb,
    pub bands: f32,
    pub rock: Rgb, // emissive
    pub glow_lo: Rgb,
    pub glow_hi: Rgb,
    pub atmo: Rgb,
    pub caps: f32,   // polar cap coverage
    pub clouds: f32, // white cloud cover
    pub rings: bool,
    pub orbit_band: u8, // 0 inner (hot/rocky), 1 mid, 2 outer (cold/gas) — for placement
}

/// A neutral terrestrial base; spread it (`BodyKind { name, .., ..base() }`) so
/// a table row only sets the fields it cares about.
pub const fn base() -> BodyKind {
    BodyKind {
        name: "",
        kind: KIND_TERRA,
        freq: 2.4,
        contr: 1.9,
        stops: &[],
        band_lo: [0.5, 0.4, 0.3],
        band_hi: [0.85, 0.78, 0.6],
        bands: 10.0,
        rock: [0.15, 0.09, 0.07],
        glow_lo: [1.0, 0.42, 0.06],
        glow_hi: [1.0, 0.92, 0.35],
        atmo: [0.0, 0.0, 0.0],
        caps: 0.0,
        clouds: 0.0,
        rings: false,
        orbit_band: 1,
    }
}

/// Per-pixel surface shade. Returns `(colour, emissive)` — the emissive factor
/// is non-zero only for [`KIND_EMISSIVE`] worlds (their self-lit lava floor);
/// every other kind returns `0.0`, so callers that never use emissive worlds
/// can ignore it.
pub fn body_surface(pk: &BodyKind, sx: f32, sy: f32, sz: f32, ofs: [f32; 3], spin_t: f32) -> (Rgb, f32) {
    let (px, py, pz) = (sx + ofs[0], sy + ofs[1], sz + ofs[2]);
    match pk.kind {
        KIND_GAS => {
            // Latitude bands with a little worley turbulence; a slow zonal drift.
            let turb = (worley(px * 3.0, py * 3.0, pz * 3.0) - 0.5) * 0.5;
            let lat = sy + turb * 0.4;
            let band = 0.5 + 0.5 * (lat * pk.bands + spin_t * 0.2).sin();
            let mut col = mix(pk.band_lo, pk.band_hi, band);
            let fine = fbm(px * 4.0, py * 4.0, pz * 4.0, 3);
            col = mix(col, pk.band_hi, smoothstep(0.55, 0.82, fine) * 0.3);
            (col, 0.0)
        }
        KIND_EMISSIVE => {
            let n = contrast(fbm(px * pk.freq, py * pk.freq, pz * pk.freq, 6), 1.7);
            let flow = fbm(px * 2.2 + spin_t * 0.5, py * 2.2, pz * 2.2, 3);
            let glow = clamp01(smoothstep(0.44, 0.66, n) * (0.55 + 0.9 * flow));
            let gcol = mix(pk.glow_lo, pk.glow_hi, clamp01(n * 1.4));
            (mix(pk.rock, gcol, glow), glow)
        }
        KIND_ICE => {
            let raw = fbm(px * pk.freq, py * pk.freq, pz * pk.freq, 5);
            let n = 1.0 - (2.0 * raw - 1.0).abs(); // ridged fractures
            let h = contrast(n, pk.contr);
            (ramp(pk.stops, h), 0.0)
        }
        _ => {
            // Terrestrial: fbm continents, sea level built into the ramp, caps.
            let raw = fbm(px * pk.freq, py * pk.freq, pz * pk.freq, 6);
            let h = contrast(raw, pk.contr);
            let mut col = ramp(pk.stops, h);
            let cap = smoothstep(0.72, 0.9, sy.abs()) * pk.caps;
            col = mix(col, [0.92, 0.95, 1.0], cap);
            (col, 0.0)
        }
    }
}

/// Render a planet to an RGBA tile, lit from world-space direction `light`
/// (already rotated into the tile's screen frame: +x right, +y up, +z toward
/// viewer). `spin_a` turns the surface; `spin_t` drives cloud/lava drift. Ring
/// worlds (`pk.rings`) get a ring plane; everything else skips it.
pub fn render_body_tile(pk: &BodyKind, seed: u32, spin_a: f32, spin_t: f32, light: [f32; 3], rad_px: f32) -> Tile {
    // Ring worlds need extra margin for the ring plane.
    let ring_margin = if pk.rings { rad_px * 1.4 } else { 1.5 };
    let size = ((rad_px + ring_margin) * 2.0).ceil() as u32;
    let size = size.max(6);
    let c = size as f32 / 2.0;
    let ofs = seed_offsets(seed, 220.0);
    let (sina, cosa) = spin_a.sin_cos();
    let has_atmo = pk.atmo != [0.0, 0.0, 0.0];
    let l = light;
    let mut px = vec![0u8; (size * size * 4) as usize];

    // Ring geometry (world tilt shared with orbits: squashed vertically).
    const RING_SQUASH: f32 = 0.42;
    let (ring_in, ring_out) = (1.28f32, 2.05f32);
    let ring_col: Rgb = [0.82, 0.74, 0.58];

    for iy in 0..size {
        for ix in 0..size {
            let nx = (ix as f32 + 0.5 - c) / rad_px;
            let ny = (c - (iy as f32 + 0.5)) / rad_px;
            let d2 = nx * nx + ny * ny;

            let mut o: Rgb = [0.0, 0.0, 0.0];
            let mut a: f32 = 0.0;

            if d2 <= 1.0 {
                let nz = (1.0 - d2).sqrt();
                // Rotate surface point around Y by the spin so it turns.
                let sx = nx * cosa + nz * sina;
                let sy = ny;
                let sz = -nx * sina + nz * cosa;

                let (mut col, emis) = body_surface(pk, sx, sy, sz, ofs, spin_t);

                if pk.clouds > 0.0 {
                    let (cs, cc) = (spin_a * 1.4).sin_cos();
                    let cx3 = nx * cc + nz * cs + ofs[0];
                    let cz3 = -nx * cs + nz * cc + ofs[2];
                    let cloud = fbm(cx3 * 2.8, ny * 2.8 + ofs[1], cz3 * 2.8 + spin_t * 0.1, 4);
                    col = mix(col, [1.0, 1.0, 1.0], smoothstep(0.54, 0.72, cloud) * pk.clouds);
                }

                // Lambert against the sun direction (emissive worlds self-light).
                let diff = (nx * l[0] + ny * l[1] + nz * l[2]).max(0.0);
                let shade = (0.08 + 0.92 * diff).max(emis);
                o = [col[0] * shade, col[1] * shade, col[2] * shade];

                // Atmospheric rim on the lit limb.
                if has_atmo {
                    let rim = (1.0 - nz).powf(3.0) * 0.6 * (0.4 + 0.6 * diff);
                    o = [
                        clamp01(o[0] + pk.atmo[0] * rim),
                        clamp01(o[1] + pk.atmo[1] * rim),
                        clamp01(o[2] + pk.atmo[2] * rim),
                    ];
                }
                a = 1.0;

                // Crisp dark limb outline for sprite readability.
                let edge = 1.0 - 1.4 / rad_px;
                if d2 > edge * edge {
                    o = [o[0] * 0.30, o[1] * 0.30, o[2] * 0.34];
                }
            }

            // Rings: draw the back half behind the disc region we've filled; the
            // front half (lower screen, ny<0) draws over. Since tiles composite
            // as a unit, we just paint ring pixels wherever the disc is empty,
            // plus the front arc even over the disc.
            if pk.rings {
                let rr = (nx * nx + (ny / RING_SQUASH).powi(2)).sqrt();
                if rr >= ring_in && rr <= ring_out && (ny < 0.0 || d2 > 1.0) {
                    let rn = (rr - ring_in) / (ring_out - ring_in);
                    let stripes = 0.5 + 0.5 * (rn * 34.0).sin();
                    let mut alpha = clamp01(0.35 + 0.5 * stripes);
                    if rn > 0.46 && rn < 0.54 {
                        alpha *= 0.14; // Cassini-ish gap
                    }
                    // Light the ring by the sun too (front side brighter).
                    let rlit = 0.5 + 0.5 * l[1].abs();
                    let rb = (0.55 + 0.45 * stripes) * rlit;
                    let rc = [ring_col[0] * rb, ring_col[1] * rb, ring_col[2] * rb];
                    o = [lerp(o[0], rc[0], alpha), lerp(o[1], rc[1], alpha), lerp(o[2], rc[2], alpha)];
                    a = a.max(alpha);
                }
            }

            let q = quant(o, bayer(ix, iy), 24.0, 0.7);
            let idx = ((iy * size + ix) * 4) as usize;
            px[idx] = (q[0] * 255.0) as u8;
            px[idx + 1] = (q[1] * 255.0) as u8;
            px[idx + 2] = (q[2] * 255.0) as u8;
            px[idx + 3] = (clamp01(a) * 255.0) as u8;
        }
    }
    Tile { px, size }
}
