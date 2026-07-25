//! noise-core — the single source of truth for the procedural-noise and
//! color-math primitives shared by every celestial crate.
//!
//! Pure math, zero dependencies. These were previously copy-pasted (byte for
//! byte) into planet/star/solar/moon/comet/asteroid; the values are unchanged,
//! so every caller renders identically.

// ---------------------------------------------------------------------------
// Noise: 3D value-noise fBm + 3D Worley (cellular) for craters.
// ---------------------------------------------------------------------------

pub fn hash3(x: i32, y: i32, z: i32) -> f32 {
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

pub fn smoother(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

pub fn value_noise(x: f32, y: f32, z: f32) -> f32 {
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

pub fn fbm(mut x: f32, mut y: f32, mut z: f32, octaves: u32) -> f32 {
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

/// Domain-warped fBm (Inigo Quilez): `fbm(p + w·fbm(p'))`. The inner noise
/// distorts the domain of the outer, turning plain bands/clouds into curling,
/// marbled, fluid-looking structure. One warp level = 4 fbm calls.
pub fn fbm_warp(x: f32, y: f32, z: f32, octaves: u32, w: f32) -> f32 {
    let qx = fbm(x, y, z, octaves);
    let qy = fbm(x + 3.1, y + 1.7, z + 5.2, octaves);
    let qz = fbm(x + 8.3, y + 2.8, z + 1.1, octaves);
    fbm(x + w * qx, y + w * qy, z + w * qz, octaves)
}

/// 3D Worley F1: distance to nearest hashed feature point. ~[0, 1.0].
pub fn worley(x: f32, y: f32, z: f32) -> f32 {
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

/// Bounded, decorrelated noise offsets from a seed. These MUST stay small: huge
/// sample coordinates lose f32 precision and the noise collapses into
/// horizontal bands (the "circular planet" bug with large random seeds). The
/// `span` sets how far seeds spread — planet/star use 256.0, the scene crates
/// (solar/moon/comet/asteroid) use 220.0.
pub fn seed_offsets(seed: u32, span: f32) -> [f32; 3] {
    [
        hash3(seed as i32, 1, 7) * span + 4.0,
        hash3(seed as i32, 2, 7) * span + 4.0,
        hash3(seed as i32, 3, 7) * span + 4.0,
    ]
}

// ---------------------------------------------------------------------------
// Color helpers
// ---------------------------------------------------------------------------

pub type Rgb = [f32; 3];

pub fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)]
}
pub fn clamp01(x: f32) -> f32 {
    x.max(0.0).min(1.0)
}
pub fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp01((x - e0) / (e1 - e0));
    t * t * (3.0 - 2.0 * t)
}
pub fn contrast(h: f32, k: f32) -> f32 {
    clamp01((h - 0.5) * k + 0.5)
}
pub fn ramp(stops: &[(f32, Rgb)], h: f32) -> Rgb {
    for s in stops {
        if h < s.0 {
            return s.1;
        }
    }
    stops[stops.len() - 1].1
}
/// Three-stop cool → mid → hot ramp.
pub fn ramp3(a: Rgb, b: Rgb, c: Rgb, t: f32) -> Rgb {
    if t < 0.5 {
        mix(a, b, t * 2.0)
    } else {
        mix(b, c, (t - 0.5) * 2.0)
    }
}
/// Palette cycling: loop through a 3-stop gradient by phase, lo→mid→hi→lo.
pub fn cycle3(lo: Rgb, mid: Rgb, hi: Rgb, phase: f32) -> Rgb {
    let p = phase.rem_euclid(1.0) * 3.0;
    if p < 1.0 {
        mix(lo, mid, p)
    } else if p < 2.0 {
        mix(mid, hi, p - 1.0)
    } else {
        mix(hi, lo, p - 2.0)
    }
}
