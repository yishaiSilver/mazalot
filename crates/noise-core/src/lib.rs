//! noise-core — the single source of truth for the procedural-noise and
//! color-math primitives shared by every celestial crate.
//!
//! Pure math, zero dependencies. These were previously copy-pasted (byte for
//! byte) into planet/star/solar/moon/comet/asteroid; the values are unchanged,
//! so every caller renders identically.

// ---------------------------------------------------------------------------
// Noise: 3D value-noise fBm + 3D Worley (cellular) for craters.
// ---------------------------------------------------------------------------

// The three axis weights and the Murmur3-style avalanche, split out so
// `value_noise` can weight each axis ONCE and reuse it across the eight corners
// that share it. Same constants, same order of operations, same bits.
const HX: u32 = 0x8da6_b343;
const HY: u32 = 0xd816_3841;
const HZ: u32 = 0xcb1a_b31f;

/// `1 / (u32::MAX as f32)`, as an exact power of two.
///
/// `u32::MAX` has no f32 representation — the nearest is 2^32 — so
/// `u32::MAX as f32` IS 2^32, and 2^-32 is exact. Multiplying by it therefore
/// gives bit-for-bit what dividing by `u32::MAX as f32` gave, and a compiler
/// cannot make that substitution itself (it can't know the reciprocal is exact
/// without fast-math). This matters: the divide was the single most expensive
/// instruction in the noise, run eight times per `value_noise` and 81 times per
/// `worley`, i.e. hundreds of times per shaded pixel.
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
    // The eight lattice corners are the eight XOR combinations of two weighted
    // values per axis, so weight each axis once instead of re-multiplying inside
    // eight `hash3` calls: 24 multiplies become 6. Bit-for-bit the same corners.
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
pub fn fbm_warp(x: f32, y: f32, z: f32, octaves: u32, w: f32) -> f32 {
    fbm_warp_oct(x, y, z, octaves, octaves, w)
}

/// [`fbm_warp`] with the warp field's octave count set separately from the
/// final field's.
///
/// The three warp components only *displace* the sample point, by at most `w`
/// domain units total — so an octave of the warp field moves the sample by
/// `w · 0.5^k`, which at the fourth octave is under 2% of a lattice cell. That
/// is far below anything the final field can resolve, let alone anything that
/// survives the ordered dither, so the warp is the cheap half to shorten:
/// `warp_oct = 2, main_oct = 4` costs 10 octave evaluations where a flat
/// `fbm_warp(.., 4, ..)` costs 16, for a visually identical field.
///
/// `warp_oct == main_oct` reproduces [`fbm_warp`] exactly.
pub fn fbm_warp_oct(x: f32, y: f32, z: f32, warp_oct: u32, main_oct: u32, w: f32) -> f32 {
    let qx = fbm(x, y, z, warp_oct);
    let qy = fbm(x + 3.1, y + 1.7, z + 5.2, warp_oct);
    let qz = fbm(x + 8.3, y + 2.8, z + 1.1, warp_oct);
    fbm(x + w * qx, y + w * qy, z + w * qz, main_oct)
}

/// 3D Worley F1: distance to nearest hashed feature point. ~[0, 1.0].
///
/// Visits the 27 cells around the sample, but prices each one first: a feature
/// point lives somewhere inside its own cell, so the distance to the cell's box
/// is a lower bound on the distance to its point. A cell whose box is already
/// further than the best distance so far cannot win, and its three hashes are
/// skipped. The centre cell is done first precisely so that bound has something
/// tight to test against — its point is usually the nearest, and most of the
/// outer ring then falls away. The result is unchanged: every cell skipped is
/// one whose `min` was provably a no-op.
pub fn worley(x: f32, y: f32, z: f32) -> f32 {
    let (fx, fy, fz) = (x.floor() as i32, y.floor() as i32, z.floor() as i32);
    // Distance from the sample to a cell's feature point.
    let dist = |cx: i32, cy: i32, cz: i32| {
        let ox = hash3(cx, cy, cz);
        let oy = hash3(cx + 911, cy + 733, cz + 512);
        let oz = hash3(cx + 271, cy + 619, cz + 188);
        let (px, py, pz) = (cx as f32 + ox, cy as f32 + oy, cz as f32 + oz);
        ((px - x).powi(2) + (py - y).powi(2) + (pz - z).powi(2)).sqrt()
    };
    // Distance from a coordinate to the cell's half-open unit interval.
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

    /// The eight-corner rewrite in `value_noise` weights each axis once and
    /// XORs, instead of calling `hash3` eight times. That is only a valid
    /// transformation because the axis weights are XORed together *before* the
    /// avalanche — pin it, because the whole workspace's imagery hangs off this
    /// function and a drift here would silently repaint every planet.
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
        // Negative and fractional coordinates included: the axis weights go
        // through `as u32` on a negative `i32`, which is where a rewrite like
        // this would most plausibly diverge.
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

    /// The pruned `worley` must equal the exhaustive 27-cell scan everywhere.
    /// Skipping a cell is only sound because the box distance is a true lower
    /// bound; an off-by-one there would carve visible notches out of every
    /// crater rim in the workspace.
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
        // Walk a fine grid across several cells, including negatives and points
        // sitting exactly on a lattice boundary.
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
    /// The reciprocal-multiply in `avalanche` is only bit-identical to the
    /// divide it replaced because `u32::MAX as f32` rounds to exactly 2^32.
    /// Assert that rounding, and the equivalence over the whole u32 range's
    /// worth of interesting keys, rather than trusting the comment.
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

    #[test]
    fn equal_octave_warp_matches_the_flat_form() {
        for k in 1..6u32 {
            for i in 0..17i32 {
                let (x, y, z) = (i as f32 * 0.61, 4.0 - i as f32 * 0.13, i as f32 * 0.29);
                assert_eq!(
                    fbm_warp(x, y, z, k, 0.8).to_bits(),
                    fbm_warp_oct(x, y, z, k, k, 0.8).to_bits()
                );
            }
        }
    }
}
