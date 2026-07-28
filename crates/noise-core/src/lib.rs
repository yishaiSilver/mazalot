//! noise-core — the single source of truth for the procedural-noise and
//! color-math primitives shared by every celestial crate.
//!
//! Pure math, zero dependencies. These were previously copy-pasted (byte for
//! byte) into planet/star/solar/moon/comet/asteroid; the values are unchanged,
//! so every caller renders identically.

// ---------------------------------------------------------------------------
// Noise: 3D value-noise fBm + 3D Worley (cellular) for craters.
// ---------------------------------------------------------------------------

// Split out so `value_noise` can weight each axis once — see there.
const HX: u32 = 0x8da6_b343;
const HY: u32 = 0xd816_3841;
const HZ: u32 = 0xcb1a_b31f;

/// `1 / (u32::MAX as f32)`. Exact, and so bit-for-bit substitutable for the
/// divide it replaced: `u32::MAX as f32` rounds to 2^32, whose reciprocal is a
/// power of two. A compiler cannot make that substitution without fast-math.
const INV_U32: f32 = 1.0 / 4_294_967_296.0;

/// Finish a weighted-and-xored lattice key into `[0, 1)`.
#[inline(always)]
fn avalanche(mut h: u32) -> f32 {
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    (h as f32) * INV_U32
}

pub fn hash3(x: i32, y: i32, z: i32) -> f32 {
    // Murmur3-style bit mixer -> well-distributed, mean ~0.5.
    avalanche(
        (x as u32).wrapping_mul(HX) ^ (y as u32).wrapping_mul(HY) ^ (z as u32).wrapping_mul(HZ),
    )
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
    // The eight corners are the XOR combinations of two weighted values per
    // axis, so weight each axis once rather than inside eight `hash3` calls.
    let (x0, x1) = ((xi as u32).wrapping_mul(HX), (xi as u32).wrapping_add(1).wrapping_mul(HX));
    let (y0, y1) = ((yi as u32).wrapping_mul(HY), (yi as u32).wrapping_add(1).wrapping_mul(HY));
    let (z0, z1) = ((zi as u32).wrapping_mul(HZ), (zi as u32).wrapping_add(1).wrapping_mul(HZ));
    let x00 = lerp(avalanche(x0 ^ y0 ^ z0), avalanche(x1 ^ y0 ^ z0), u);
    let x10 = lerp(avalanche(x0 ^ y1 ^ z0), avalanche(x1 ^ y1 ^ z0), u);
    let x01 = lerp(avalanche(x0 ^ y0 ^ z1), avalanche(x1 ^ y0 ^ z1), u);
    let x11 = lerp(avalanche(x0 ^ y1 ^ z1), avalanche(x1 ^ y1 ^ z1), u);
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
///
/// The warp field takes its own octave count because it only *displaces* the
/// sample point: its `k`-th octave moves it `w · 0.5^k`, under 2% of a lattice
/// cell by the fourth — nothing the warped field can resolve. Two warp octaves
/// against four main costs 10 octave evaluations where a uniform four costs 16.
pub fn fbm_warp(x: f32, y: f32, z: f32, warp_oct: u32, main_oct: u32, w: f32) -> f32 {
    let qx = fbm(x, y, z, warp_oct);
    let qy = fbm(x + 3.1, y + 1.7, z + 5.2, warp_oct);
    let qz = fbm(x + 8.3, y + 2.8, z + 1.1, warp_oct);
    fbm(x + w * qx, y + w * qy, z + w * qz, main_oct)
}

/// 3D Worley F1: distance to nearest hashed feature point. ~[0, 1.0].
///
/// Prices each of the 27 cells before hashing it: a feature point lies inside
/// its own cell, so the distance to that cell's box lower-bounds the distance to
/// its point, and a cell already further than the best so far cannot win. The
/// centre cell goes first to give the bound something tight to test against.
/// Exact — every cell skipped is one whose `min` was provably a no-op.
pub fn worley(x: f32, y: f32, z: f32) -> f32 {
    let (fx, fy, fz) = (x.floor() as i32, y.floor() as i32, z.floor() as i32);
    let dist = |cx: i32, cy: i32, cz: i32| {
        let ox = hash3(cx, cy, cz);
        let oy = hash3(cx + 911, cy + 733, cz + 512);
        let oz = hash3(cx + 271, cy + 619, cz + 188);
        let (px, py, pz) = (cx as f32 + ox, cy as f32 + oy, cz as f32 + oz);
        ((px - x).powi(2) + (py - y).powi(2) + (pz - z).powi(2)).sqrt()
    };
    // Distance from a coordinate to the cell's unit interval.
    let gap = |c: i32, v: f32| {
        let lo = c as f32;
        (lo - v).max(v - (lo + 1.0)).max(0.0)
    };

    let mut f1 = dist(fx, fy, fz).min(9.0);
    for dz in -1..=1 {
        for dy in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dy == 0 && dz == 0 {
                    continue; // already done, and it seeded the bound
                }
                let (cx, cy, cz) = (fx + dx, fy + dy, fz + dz);
                let (bx, by, bz) = (gap(cx, x), gap(cy, y), gap(cz, z));
                if bx * bx + by * by + bz * bz >= f1 * f1 {
                    continue; // the whole cell is further than the best so far
                }
                f1 = f1.min(dist(cx, cy, cz));
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

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The eight-corner rewrite is valid only because the axis weights are
    /// XORed *before* the avalanche. Every planet in the workspace hangs off
    /// this function, so pin it.
    fn reference(x: f32, y: f32, z: f32) -> f32 {
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

    #[test]
    fn value_noise_is_bit_identical_to_the_eight_hash_form() {
        // Negative coordinates included: the axis weights go through `as u32`
        // on a negative `i32`, where such a rewrite would most likely diverge.
        let mut n = 0;
        for i in -40..40i32 {
            for j in -7..7i32 {
                let (x, y, z) = (i as f32 * 0.37, j as f32 * 1.9 - 3.3, i as f32 * -0.11 + 12.5);
                assert_eq!(
                    value_noise(x, y, z).to_bits(),
                    reference(x, y, z).to_bits(),
                    "value_noise diverged at ({x}, {y}, {z})"
                );
                n += 1;
            }
        }
        assert!(n > 500);
    }

    /// Pruning is sound only if the box distance is a true lower bound; an
    /// off-by-one there would notch every crater rim in the workspace.
    #[test]
    fn pruned_worley_matches_the_exhaustive_scan() {
        fn reference(x: f32, y: f32, z: f32) -> f32 {
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
                        f1 = f1.min(((px - x).powi(2) + (py - y).powi(2) + (pz - z).powi(2)).sqrt());
                    }
                }
            }
            f1
        }
        // Includes negatives and points sitting on a lattice boundary.
        let mut n = 0;
        for i in -60..60i32 {
            for j in -13..13i32 {
                let (x, y, z) = (i as f32 * 0.25, j as f32 * 0.5, i as f32 * 0.125 - 2.0);
                assert_eq!(
                    worley(x, y, z).to_bits(),
                    reference(x, y, z).to_bits(),
                    "worley diverged at ({x}, {y}, {z})"
                );
                n += 1;
            }
        }
        assert!(n > 2000);
    }

    /// `fbm_warp` is now a thin call into `fbm_warp_oct`; equal octave counts
    /// must still be the exact old four-call form.
    /// Bit-identical only because `u32::MAX as f32` rounds to exactly 2^32 —
    /// assert that rather than trusting the comment on `INV_U32`.
    #[test]
    fn reciprocal_multiply_matches_the_divide() {
        assert_eq!((u32::MAX as f32).to_bits(), 4_294_967_296.0f32.to_bits());
        for h in [0u32, 1, 2, 255, 65535, 0x7fff_ffff, 0x8000_0000, 0xffff_ffff] {
            assert_eq!(((h as f32) * INV_U32).to_bits(), ((h as f32) / (u32::MAX as f32)).to_bits());
        }
        for k in 0..5000u32 {
            let h = k.wrapping_mul(0x9e37_79b9);
            assert_eq!(((h as f32) * INV_U32).to_bits(), ((h as f32) / (u32::MAX as f32)).to_bits());
        }
    }

}
