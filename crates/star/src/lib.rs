//! star — the single source of truth for procedural star generation.
//!
//! Pure math, zero dependencies. Produces raw RGBA bytes via [`render_rgba`];
//! callers wrap those however they like (the native bin turns them into
//! GIFs/PNGs with the `image` crate; a wasm face can expose them to a canvas).
//!
//! A star is the *inverse* of a planet: it is self-luminous, so there is no
//! day/night terminator and no external light. What sells "this is a sun" is a
//! churning **granulation** field (convection cells with dark inter-granular
//! lanes), drifting **sunspots**, **limb darkening** toward the cooler edge, a
//! soft **corona** halo, and a continuous shimmer of **prominences** off the
//! limb. Same inputs => same star; a full 2π `angle` loop is seamless.
//!
//! This crate is self-contained by design (the workspace's rule: each "type"
//! crate shares no code with the others). The noise/color/dither primitives
//! below are the star-relevant subset of the same toolkit `planet` uses.

use std::f32::consts::{PI, TAU};

// ---------------------------------------------------------------------------
// Noise: 3D value-noise fBm + 3D Worley (cellular) for convection cells.
// ---------------------------------------------------------------------------

fn hash3(x: i32, y: i32, z: i32) -> f32 {
    // Murmur3-style bit mixer -> well-distributed, mean ~0.5.
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ (z as u32).wrapping_mul(0xcb1a_b31f);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    (h as f32) / (u32::MAX as f32)
}

fn smoother(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn value_noise(x: f32, y: f32, z: f32) -> f32 {
    let (xi, yi, zi) = (x.floor(), y.floor(), z.floor());
    let (xf, yf, zf) = (x - xi, y - yi, z - zi);
    let (xi, yi, zi) = (xi as i32, yi as i32, zi as i32);
    let (u, v, w) = (smoother(xf), smoother(yf), smoother(zf));
    let c = |dx: i32, dy: i32, dz: i32| hash3(xi + dx, yi + dy, zi + dz);
    let x00 = lerp(c(0, 0, 0), c(1, 0, 0), u);
    let x10 = lerp(c(0, 1, 0), c(1, 1, 0), u);
    let x01 = lerp(c(0, 0, 1), c(1, 0, 1), u);
    let x11 = lerp(c(0, 1, 1), c(1, 1, 1), u);
    lerp(lerp(x00, x10, v), lerp(x01, x11, v), w)
}

fn fbm(mut x: f32, mut y: f32, mut z: f32, octaves: u32) -> f32 {
    let (mut sum, mut amp, mut norm) = (0.0, 0.5, 0.0);
    for _ in 0..octaves {
        sum += amp * value_noise(x, y, z);
        norm += amp;
        amp *= 0.5;
        x *= 2.0;
        y *= 2.0;
        z *= 2.0;
    }
    sum / norm
}

/// 3D Worley F1: distance to nearest hashed feature point. ~[0, 1.0].
fn worley(x: f32, y: f32, z: f32) -> f32 {
    let (fx, fy, fz) = (x.floor() as i32, y.floor() as i32, z.floor() as i32);
    let mut f1 = 9.0f32;
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                let (cx, cy, cz) = (fx + dx, fy + dy, fz + dz);
                let ox = hash3(cx, cy, cz);
                let oy = hash3(cx + 911, cy + 733, cz + 512);
                let oz = hash3(cx + 271, cy + 619, cz + 188);
                let (px, py, pz) = (cx as f32 + ox, cy as f32 + oy, cz as f32 + oz);
                let d = ((px - x).powi(2) + (py - y).powi(2) + (pz - z).powi(2)).sqrt();
                f1 = f1.min(d);
            }
        }
    }
    f1
}

// ---------------------------------------------------------------------------
// 5D value noise — for seamless "boil-in-place" over the loop.
//
// To evolve a field over time AND return to itself at θ=2π without drift or
// ease-in/out, the time axis must trace a closed loop in noise space. The
// minimal such loop is a circle, which needs TWO extra dimensions. Our surface
// pattern already lives on a 3D sphere point, so a looping-in-place boil is 3
// spatial + 2 time = 5 dimensions. (A single scalar time axis can only go
// back-and-forth, which is exactly the rigid, reversing motion we're replacing.)
// ---------------------------------------------------------------------------

fn hash5(x: i32, y: i32, z: i32, w: i32, v: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343)
        ^ (y as u32).wrapping_mul(0xd816_3841)
        ^ (z as u32).wrapping_mul(0xcb1a_b31f)
        ^ (w as u32).wrapping_mul(0x1656_67b1)
        ^ (v as u32).wrapping_mul(0x27d4_eb2f);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    (h as f32) / (u32::MAX as f32)
}

/// 5D value noise via multilinear interpolation of the 32 hypercube corners.
fn value_noise_5d(p: [f32; 5]) -> f32 {
    let mut i = [0i32; 5];
    let mut u = [0f32; 5];
    for k in 0..5 {
        let fl = p[k].floor();
        i[k] = fl as i32;
        u[k] = smoother(p[k] - fl);
    }
    let mut sum = 0.0f32;
    for m in 0..32u32 {
        let mut wgt = 1.0f32;
        let mut c = [0i32; 5];
        for k in 0..5 {
            if (m >> k) & 1 == 1 {
                c[k] = i[k] + 1;
                wgt *= u[k];
            } else {
                c[k] = i[k];
                wgt *= 1.0 - u[k];
            }
        }
        sum += wgt * hash5(c[0], c[1], c[2], c[3], c[4]);
    }
    sum
}

/// fBm over 5D value noise.
fn fbm5(mut p: [f32; 5], octaves: u32) -> f32 {
    let (mut sum, mut amp, mut norm) = (0.0, 0.5, 0.0);
    for _ in 0..octaves {
        sum += amp * value_noise_5d(p);
        norm += amp;
        amp *= 0.5;
        for e in p.iter_mut() {
            *e *= 2.0;
        }
    }
    sum / norm
}

// ---------------------------------------------------------------------------
// Color helpers
// ---------------------------------------------------------------------------

type Rgb = [f32; 3];

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)]
}
fn clamp01(x: f32) -> f32 {
    x.max(0.0).min(1.0)
}
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp01((x - e0) / (e1 - e0));
    t * t * (3.0 - 2.0 * t)
}

/// Three-stop cool → mid → hot temperature ramp.
fn ramp3(a: Rgb, b: Rgb, c: Rgb, t: f32) -> Rgb {
    if t < 0.5 {
        mix(a, b, t * 2.0)
    } else {
        mix(b, c, (t - 0.5) * 2.0)
    }
}

/// Bounded, decorrelated noise offsets from a seed. These MUST stay small: huge
/// sample coordinates lose f32 precision and the noise collapses into bands.
fn seed_offsets(seed: u32) -> [f32; 3] {
    [
        hash3(seed as i32, 1, 7) * 256.0 + 4.0,
        hash3(seed as i32, 2, 7) * 256.0 + 4.0,
        hash3(seed as i32, 3, 7) * 256.0 + 4.0,
    ]
}

// ---------------------------------------------------------------------------
// Pixel-art output: ordered (Bayer) dithering.
// ---------------------------------------------------------------------------

/// Global look settings (not per-type). Kept as a struct so a future web face
/// can tune it live.
pub struct Style {
    pub dither: f32, // 0..1 ordered-dither strength
}
impl Style {
    pub fn natural() -> Style {
        Style { dither: 0.7 }
    }
}

// 8x8 ordered (Bayer) matrix, values 0..63.
const BAYER: [u8; 64] = [
    0, 32, 8, 40, 2, 34, 10, 42, 48, 16, 56, 24, 50, 18, 58, 26, 12, 44, 4, 36, 14, 46,
    6, 38, 60, 28, 52, 20, 62, 30, 54, 22, 3, 35, 11, 43, 1, 33, 9, 41, 51, 19, 59, 27,
    49, 17, 57, 25, 15, 47, 7, 39, 13, 45, 5, 37, 63, 31, 55, 23, 61, 29, 53, 21,
];
fn bayer(x: u32, y: u32) -> f32 {
    (BAYER[((y % 8) * 8 + (x % 8)) as usize] as f32 + 0.5) / 64.0 - 0.5 // -0.5..0.5
}

/// Final per-pixel quantization via ordered dithering — kills ramp banding and
/// dithers the corona falloff while staying crisp under spin.
fn finalize(o: Rgb, bx: f32, style: &Style) -> Rgb {
    let levels = 22.0;
    let d = bx * style.dither / levels;
    [
        clamp01(((o[0] + d) * levels).round() / levels),
        clamp01(((o[1] + d) * levels).round() / levels),
        clamp01(((o[2] + d) * levels).round() / levels),
    ]
}

// ---------------------------------------------------------------------------
// Star type table
// ---------------------------------------------------------------------------

/// One star archetype. Mostly a small palette (cool → mid → hot photosphere,
/// plus corona/prominence tints) with a few behavioural knobs.
#[derive(Clone, Copy)]
pub struct SType {
    name: &'static str,
    cool: Rgb,        // coolest granules / limb
    mid: Rgb,         // main photosphere
    hot: Rgb,         // brightest granule cores
    spot_col: Rgb,    // sunspot umbra
    flare: Rgb,       // prominence / flare tint
    gran_freq: f32,   // granulation cell frequency (bigger = finer cells)
    turb: f32,        // domain-warp turbulence in the granulation
    spots: f32,       // sunspot coverage/darkness
    activity: f32,    // steady prominence intensity
    flicker: f32,     // extra randomized flare-star spikes
    corona_size: f32, // how far the halo reaches past the limb
    fur: f32,         // prominence fur density (bigger = finer, denser spikes)
    radius_scale: f32,
}

const fn base() -> SType {
    SType {
        name: "",
        cool: [0.55, 0.20, 0.02],
        mid: [0.98, 0.72, 0.18],
        hot: [1.0, 0.97, 0.82],
        spot_col: [0.30, 0.12, 0.03],
        flare: [1.0, 0.85, 0.50],
        gran_freq: 6.5,
        turb: 0.8,
        spots: 0.6,
        activity: 0.7,
        flicker: 0.0,
        corona_size: 1.0,
        fur: 1.0,
        radius_scale: 1.0,
    }
}

/// The star spectrum, hot → cool, then a couple of exotics. One row per type.
pub const STYPES: &[SType] = &[
    // O/B blue giant — huge, violently active, blue-white.
    SType {
        name: "blue_giant",
        cool: [0.10, 0.22, 0.60], mid: [0.45, 0.62, 1.0], hot: [0.92, 0.97, 1.0],
        spot_col: [0.06, 0.10, 0.30], flare: [0.75, 0.88, 1.0],
        gran_freq: 5.5, turb: 1.0, spots: 0.35, activity: 1.0, corona_size: 1.35, fur: 1.1,
        radius_scale: 1.18, ..base()
    },
    // A-type white star — clean blue-white, low activity.
    SType {
        name: "white_star",
        cool: [0.48, 0.56, 0.85], mid: [0.85, 0.90, 1.0], hot: [1.0, 1.0, 1.0],
        spot_col: [0.30, 0.36, 0.55], flare: [0.88, 0.94, 1.0],
        gran_freq: 7.5, turb: 0.7, spots: 0.25, activity: 0.5, corona_size: 0.95, fur: 1.0,
        ..base()
    },
    // G-type yellow dwarf — our Sun. The default palette.
    SType {
        name: "yellow_dwarf",
        cool: [0.55, 0.20, 0.02], mid: [0.98, 0.72, 0.18], hot: [1.0, 0.97, 0.82],
        spot_col: [0.28, 0.11, 0.02], flare: [1.0, 0.85, 0.48],
        gran_freq: 6.5, turb: 0.8, spots: 0.6, activity: 0.75, corona_size: 1.0, fur: 1.0,
        ..base()
    },
    // K-type orange dwarf — warmer, spottier.
    SType {
        name: "orange_dwarf",
        cool: [0.35, 0.08, 0.01], mid: [0.95, 0.50, 0.12], hot: [1.0, 0.85, 0.52],
        spot_col: [0.22, 0.06, 0.01], flare: [1.0, 0.70, 0.32],
        gran_freq: 6.0, turb: 0.85, spots: 0.75, activity: 0.7, corona_size: 0.95, fur: 0.95,
        radius_scale: 0.9, ..base()
    },
    // M-type red giant — bloated, cool, dramatic prominences.
    SType {
        name: "red_giant",
        cool: [0.26, 0.03, 0.02], mid: [0.86, 0.24, 0.08], hot: [1.0, 0.62, 0.28],
        spot_col: [0.16, 0.02, 0.02], flare: [1.0, 0.46, 0.20],
        gran_freq: 4.5, turb: 1.0, spots: 0.9, activity: 1.0, corona_size: 1.45, fur: 0.85,
        radius_scale: 1.22, ..base()
    },
    // M-dwarf flare star — small, dim, prone to sudden violent flares.
    SType {
        name: "red_dwarf",
        cool: [0.20, 0.02, 0.02], mid: [0.70, 0.18, 0.07], hot: [0.98, 0.46, 0.22],
        spot_col: [0.12, 0.01, 0.01], flare: [1.0, 0.55, 0.28],
        gran_freq: 8.0, turb: 0.9, spots: 0.7, activity: 0.35, flicker: 1.0, corona_size: 0.8,
        fur: 1.2, radius_scale: 0.72, ..base()
    },
    // Tiny, brilliant, hot white dwarf — dense fine granules, small halo.
    SType {
        name: "white_dwarf",
        cool: [0.55, 0.62, 0.92], mid: [0.88, 0.93, 1.0], hot: [1.0, 1.0, 1.0],
        spot_col: [0.38, 0.44, 0.62], flare: [0.85, 0.92, 1.0],
        gran_freq: 11.0, turb: 0.6, spots: 0.15, activity: 0.3, corona_size: 0.7, fur: 1.3,
        radius_scale: 0.62, ..base()
    },
    // Exotic teal star — the `rebels-in-the-sky` "sol" look that inspired this.
    SType {
        name: "sol",
        cool: [0.01, 0.24, 0.36], mid: [0.11, 0.56, 0.65], hot: [0.96, 1.0, 0.91],
        spot_col: [0.01, 0.14, 0.22], flare: [0.72, 1.0, 0.94],
        gran_freq: 7.0, turb: 0.85, spots: 0.4, activity: 0.8, corona_size: 1.1, fur: 1.15,
        ..base()
    },
];

/// Number of star types.
pub fn type_count() -> usize {
    STYPES.len()
}
/// Name of a star type (wraps on out-of-range index).
pub fn type_name(i: usize) -> &'static str {
    STYPES[i % STYPES.len()].name
}

// ---------------------------------------------------------------------------
// Surface shading
// ---------------------------------------------------------------------------

/// Photosphere temperature/brightness at a rotated surface point, in [0, 1].
/// Convection cells (worley) with a warped-fbm mottle on top, boiling over time.
fn granulation(ct: &SType, sx: f32, sy: f32, sz: f32, ofs: [f32; 3], angle: f32) -> f32 {
    let f = ct.gran_freq;
    let (px, py, pz) = (sx + ofs[0], sy + ofs[1], sz + ofs[2]);
    // Looping time as a CIRCLE in two added noise dimensions. As θ runs 0→2π the
    // point (cos θ, sin θ)·R traces a closed constant-speed loop, so the warp
    // field boils in place and returns exactly to itself — no drift, no ease.
    // R sets how vigorously it churns per loop (bigger = the field decorrelates
    // more before coming back).
    let rt = 1.25;
    let (tc, ts) = (angle.cos() * rt, angle.sin() * rt);

    // Time-evolving domain warp (5D) displaces the cell field. Because the warp
    // itself now churns through the loop, granules stretch / merge / dissolve in
    // place instead of the whole sheet sliding past — real boiling, not panning.
    let warp = 0.55 * f * 0.6;
    let wx = fbm5([px * 1.6, py * 1.6, pz * 1.6, tc, ts], 2) - 0.5;
    let wy = fbm5([px * 1.6 + 5.2, py * 1.6 + 1.3, pz * 1.6 + 9.1, tc, ts], 2) - 0.5;
    let wz = fbm5([px * 1.6 + 2.7, py * 1.6 + 8.4, pz * 1.6 + 3.9, tc, ts], 2) - 0.5;
    let (qx, qy, qz) = (px * f + warp * wx, py * f + warp * wy, pz * f + warp * wz);

    // A burning surface is mostly hot with a thin COOL network threaded through
    // it — not a dark ball with bright specks. So default bright, and let the
    // worley lanes (F1 local maxima) carve the dark inter-granular network. A
    // finer second scale adds smaller lanes.
    // The reference star is ~85% white-hot with only sparse cool blotches — so
    // start HOT and carve a MINORITY cool network in, not the reverse. A chunky
    // low-frequency field places the blotches; worley adds thin inter-granular
    // lanes for fine texture. Both ride the time-warped coords, so they boil.
    let blotch = fbm(qx * 0.55, qy * 0.55, qz * 0.55, 4);
    let cool_region = smoothstep(0.46, 0.30, blotch); // 1 inside the sparse cool blobs
    let w = worley(qx, qy, qz);
    let lane = smoothstep(0.55, 0.82, w); // thin dark lanes
    let dark = clamp01(cool_region * 0.9 + lane * 0.45);

    clamp01(1.0 - 0.9 * dark)
}

/// Sunspot darkening in [0, 1] (1 = deep umbra). Low-frequency, slowly drifting.
fn sunspot(ct: &SType, sx: f32, sy: f32, sz: f32, ofs: [f32; 3], angle: f32) -> f32 {
    if ct.spots <= 0.0 {
        return 0.0;
    }
    let drift = angle.cos() * 0.15;
    let n = fbm((sx + ofs[2]) * 1.1 + drift, (sy + ofs[0]) * 1.1, (sz + ofs[1]) * 1.1, 4);
    // Only the deepest troughs become spots, so they read as discrete blemishes.
    smoothstep(0.30, 0.16, n) * ct.spots
}

/// Continuous, organic corona-boundary height in [0, 1] as a function of the limb
/// angle `theta` and time `angle`. Synthesised from many harmonics — coarse
/// flares through fine fur — each with an INTEGER angular frequency (seamless
/// around the circle) and an INTEGER temporal frequency (seamless over the 2π
/// loop) but its own phase and rate. So the whole limb bristles with spikes at
/// *every* angle and no two of them flicker in lockstep — which is what reads as
/// "alive" rather than a few pulsing lobes.
fn spiky(theta: f32, angle: f32, seed: u32, density: f32) -> f32 {
    let mut h = 0.0f32;
    let mut norm = 0.0f32;
    for k in 0..16 {
        // Angular frequency climbs from coarse flares to fine fur. Rounded to an
        // integer so sin(theta·n) has no seam where theta wraps at ±π.
        let n = ((1.0 + k as f32 * 2.0) * density).round().max(1.0);
        let m = 1.0 + (hash3(seed as i32, k, 21) * 3.0).floor(); // temporal 1..3, integer
        let ph = hash3(seed as i32, k, 22) * TAU;
        // Flatter-than-1/n spectrum so the fine fur stays visible, not drowned by
        // the big lobes.
        let amp = (1.0 / n).powf(0.4);
        h += amp * (0.5 + 0.5 * (theta * n + angle * m + ph).sin());
        norm += amp;
    }
    h / norm // ~[0, 1], mean ~0.5
}

/// Prominences licking off the limb — deliberately NO smooth halo/aura. Three
/// layers compose the star's fire, all loop-seamless (every temporal rate is an
/// integer, so the field returns to itself over a 2π `angle`):
///   1. **fur**   — a low continuous bristle so the limb is never a clean circle;
///   2. **rays**  — thin needle jets that POP: sharp in space (≈1px wide) and in
///                  time (a high-power envelope keeps each ray dark, then briefly
///                  shoots it far out), the sharp flares the reference throws;
///   3. **loops** — a few arced prominences that rise off the limb and curve back
///                  down to a second footpoint, growing and fading.
/// Returns extra emissive RGB; `r` is the radial distance in disc radii (1.0 ==
/// the limb).
fn corona(ct: &SType, nx: f32, ny: f32, r: f32, angle: f32, seed: u32) -> Rgb {
    let edge = r - 1.0;
    let maxr = 1.5 * ct.corona_size;
    if edge <= 0.0 || edge > maxr {
        return [0.0, 0.0, 0.0]; // inside the disc, or past every jet — black
    }
    let theta = ny.atan2(nx);
    let (ctx, cty) = (theta.cos(), theta.sin()); // seam-free circle coords
    let si = seed as i32;
    let mut glow = 0.0f32;

    // (1) Baseline fur — short, continuous, ragged; keeps the limb alive between
    // the big jets so it never reads as a bare disc.
    let h = spiky(theta, angle, seed, ct.fur);
    let ragged = 0.55 + 0.8 * fbm(ctx * 11.0 * ct.fur, cty * 11.0 * ct.fur, angle.cos() * 1.4, 3);
    let fur = clamp01((h - 0.5) * 2.2).powf(1.8) * ragged;
    let fur_reach = ct.corona_size * (0.03 + 0.13 * fur);
    if edge < fur_reach {
        glow += smoothstep(fur_reach, 0.0, edge) * (0.35 + 0.7 * fur);
    }

    // (2) Sharp needle rays. Many thin slots around the limb; each fires on its
    // own integer-rate, high-power envelope so it is dark most of the time then
    // pops out crisply — the flare "popping" the reference shows frame to frame.
    {
        let rays = (34.0 * ct.fur).round().max(8.0);
        let a = (theta / TAU + 0.5) * rays;
        let slot = a.round() as i32;
        let frac = (a - a.round()).abs(); // 0..0.5, distance to this ray's centre
        let rate = 1.0 + (hash3(si, slot, 52) * 3.0).floor(); // integer => seamless
        let ph = hash3(si, slot, 53) * TAU;
        // Rays are sparse, short, sharp accents — NOT long lances competing with
        // the disc. Only a minority of slots fire (high dud threshold), briefly
        // (steep envelope), and reach only a fraction of the radius.
        let live = smoothstep(0.52, 0.74, hash3(si, slot, 51));
        let env = (0.5 + 0.5 * (angle * rate + ph).sin()).powi(12); // sharp, brief
        let jitter = 0.7 + 0.4 * fbm(ctx * 7.0, cty * 7.0, angle.sin(), 2); // ragged tip
        let len = ct.corona_size * (0.04 + 0.18 * live) * env * jitter * (1.0 + 1.0 * ct.flicker);
        let ang = smoothstep(0.30, 0.0, frac); // ≈1px-thin needle
        if len > 1e-3 && edge < len {
            let radial = smoothstep(len, 0.0, edge).powf(0.8); // crisp, not soft
            glow += ang * radial * (0.30 + 0.55 * env) * (0.6 * ct.activity + ct.flicker * env);
        }
    }

    // (3) Loop prominences — a handful of arcs. Each spans two footpoints ±we
    // around a seeded angle and rises to a time-varying height; the visible arc
    // is a thin bright curve (semi-ellipse) that grows and fades.
    for e in 0..4 {
        let te = hash3(si, e, 61) * TAU - PI;
        let mut dth = theta - te;
        while dth > PI {
            dth -= TAU;
        }
        while dth < -PI {
            dth += TAU;
        }
        let we = 0.16 + 0.16 * hash3(si, e, 62);
        if dth.abs() >= we {
            continue;
        }
        let rate = 1.0 + (hash3(si, e, 63) * 2.0).floor(); // integer => seamless
        let ph = hash3(si, e, 64) * TAU;
        let env = (0.5 + 0.5 * (angle * rate + ph).sin()).powi(3); // rise / fall
        let he = (0.22 + 0.30 * hash3(si, e, 65)) * ct.corona_size * env;
        if he <= 0.02 {
            continue;
        }
        let u = dth / we;
        let hgt = he * (1.0 - u * u).max(0.0).sqrt(); // arc top (semi-ellipse)
        let thick = 0.03 + 0.04 * he;
        let arc = smoothstep(thick, 0.0, (edge - hgt).abs());
        glow += arc * (0.9 + 1.1 * env) * (0.6 + ct.activity);
    }

    if glow <= 1e-4 {
        return [0.0, 0.0, 0.0];
    }
    // White-hot at the root, cooling to the flare tint with height.
    let col = mix([1.0, 1.0, 1.0], ct.flare, smoothstep(0.0, 0.45, edge / maxr));
    [clamp01(col[0] * glow), clamp01(col[1] * glow), clamp01(col[2] * glow)]
}

/// A blinking ember flung just off the limb (the scattered bright motes in the
/// reference). Sparse and close to the disc — embers, not a starfield aura.
fn spark(ix: u32, iy: u32, r: f32, angle: f32, seed: u32) -> f32 {
    if r <= 1.0 || r > 1.5 {
        return 0.0;
    }
    let h = hash3(ix as i32, iy as i32, seed as i32 ^ 0x51D);
    if h < 0.978 {
        return 0.0;
    }
    // Blink on and off on an individual phase, so embers flicker rather than sit.
    let tw = 0.5 + 0.5 * (angle * (2.0 + hash3(ix as i32, iy as i32, 7) * 3.0)
        + hash3(ix as i32, iy as i32, 9) * TAU).sin();
    smoothstep(0.55, 1.0, tw) * (1.0 - smoothstep(1.1, 1.5, r))
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

/// Render one star frame as RGBA into `out` (must be >= size*size*4 bytes).
/// `angle` is the animation phase in radians; a full 2π loop is seamless.
pub fn render_rgba(size: u32, type_idx: usize, seed: u32, angle: f32, out: &mut [u8]) {
    render_ct(size, &STYPES[type_idx % STYPES.len()], seed, angle, SPIN_TURNS, &Style::natural(), out);
}

/// Number of tunable parameters exposed to the web sliders (see [`param`]).
pub const NUM_PARAMS: usize = 7;

/// A tunable parameter of a star type, by index (must match [`render_rgba_params`]):
/// 0 granulation freq, 1 turbulence, 2 sunspots, 3 activity (prominences),
/// 4 flicker (flare-star spikes), 5 corona size, 6 fur (prominence density).
pub fn param(type_idx: usize, which: u32) -> f32 {
    let ct = &STYPES[type_idx % STYPES.len()];
    match which {
        0 => ct.gran_freq,
        1 => ct.turb,
        2 => ct.spots,
        3 => ct.activity,
        4 => ct.flicker,
        5 => ct.corona_size,
        6 => ct.fur,
        _ => 0.0,
    }
}

/// Render with a slider-parameter override array (`NUM_PARAMS` values, same order
/// as [`param`]) plus global `dither` and runtime `spin` (whole turns per 2π —
/// keep it an integer for a seamless loop). Used by the web demo.
#[allow(clippy::too_many_arguments)]
pub fn render_rgba_params(
    size: u32,
    type_idx: usize,
    seed: u32,
    angle: f32,
    p: &[f32],
    dither: f32,
    spin: f32,
    out: &mut [u8],
) {
    let mut ct = STYPES[type_idx % STYPES.len()];
    if p.len() >= NUM_PARAMS {
        ct.gran_freq = p[0];
        ct.turb = p[1];
        ct.spots = p[2];
        ct.activity = p[3];
        ct.flicker = p[4];
        ct.corona_size = p[5];
        ct.fur = p[6];
    }
    render_ct(size, &ct, seed, angle, spin, &Style { dither }, out);
}

/// Whole turns of rigid rotation per loop. Only INTEGER values stay seamless
/// (a fractional turn leaves the sphere in a different orientation at θ=2π, i.e.
/// a visible jump). The reference sun barely rotates — its life is the boil — so
/// the native default is 0: the surface churns in place and the 5D boil carries
/// all the motion. (The web demo can override this at runtime.)
const SPIN_TURNS: f32 = 0.0;

fn render_ct(size: u32, ct: &SType, seed: u32, angle: f32, spin: f32, style: &Style, out: &mut [u8]) {
    let (cx, cy) = (size as f32 / 2.0, size as f32 / 2.0);
    let ofs = seed_offsets(seed);
    let (sina, cosa) = (angle * spin).sin_cos();
    // Leave generous margin: the corona + prominences reach well past the disc.
    let rad = size as f32 * 0.235 * ct.radius_scale;

    for iy in 0..size {
        for ix in 0..size {
            let nx = (ix as f32 + 0.5 - cx) / rad;
            let ny = (cy - (iy as f32 + 0.5)) / rad;
            let d2 = nx * nx + ny * ny;
            let r = d2.sqrt();

            let mut o;
            if d2 <= 1.0 {
                // Sphere point, rotated around Y by SPIN_TURNS (0 => no roll).
                let nz = (1.0 - d2).sqrt();
                let sx = nx * cosa + nz * sina;
                let sy = ny;
                let sz = -nx * sina + nz * cosa;

                let t = granulation(ct, sx, sy, sz, ofs, angle);
                let mut col = ramp3(ct.cool, ct.mid, ct.hot, t);

                // Sunspots: darken toward the umbra colour.
                let spot = sunspot(ct, sx, sy, sz, ofs, angle);
                col = mix(col, ct.spot_col, spot);

                // Gentle limb darkening (mu = nz): a little dimmer + cooler at the
                // edge for a spherical read, but the surface stays bright to the
                // limb so it burns rather than fading into a dark rim.
                let mu = nz;
                let limb = 0.62 + 0.38 * mu.powf(0.45);
                col = mix(mix(col, ct.cool, 0.22 * (1.0 - mu)), col, mu.sqrt());
                o = [col[0] * limb, col[1] * limb, col[2] * limb];
            } else {
                // The star burns against black — no space-navy fill, no starfield
                // aura. Only the flame tongues and embers below light the void.
                o = [0.0, 0.0, 0.0];
            }

            // Corona + prominences glow over disc and background alike.
            let c = corona(ct, nx, ny, r, angle, seed);
            o = [clamp01(o[0] + c[0]), clamp01(o[1] + c[1]), clamp01(o[2] + c[2])];

            // Twinkling motes in the halo.
            let sp = spark(ix, iy, r, angle, seed);
            if sp > 0.0 {
                let s = sp * 0.9;
                o = [clamp01(o[0] + s), clamp01(o[1] + s), clamp01(o[2] + s)];
            }

            let px = finalize(o, bayer(ix, iy), style);
            let idx = ((iy * size + ix) * 4) as usize;
            out[idx] = (clamp01(px[0]) * 255.0) as u8;
            out[idx + 1] = (clamp01(px[1]) * 255.0) as u8;
            out[idx + 2] = (clamp01(px[2]) * 255.0) as u8;
            out[idx + 3] = 255;
        }
    }
}

// Browser (wasm) C-ABI glue — excluded from native builds. See wasm.rs.
#[cfg(target_arch = "wasm32")]
mod wasm;
