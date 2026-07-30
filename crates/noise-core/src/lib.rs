//! noise-core — the single source of truth for the procedural-noise and
//! color-math primitives shared by every celestial crate.
//!
//! Pure math, zero dependencies. These were previously copy-pasted (byte for
//! byte) into planet/star/solar/moon/comet/asteroid; the values are unchanged,
//! so every caller renders identically.
//!
//! The two lattice kernels — [`value_noise`] and [`worley`] — hash their corners
//! four at a time through the [`lanes`] shim, which maps onto wasm `simd128` in
//! the browser builds and onto plain arrays everywhere else. Both paths are
//! bit-identical to the original scalar code; see `lanes.rs` for the rules that
//! keep them that way.

mod lanes;
use lanes::{F32x4, U32x4};

/// The GLSL ES 3.00 transliteration of everything below, plus the `#version`
/// line and the precision qualifiers. Every fragment shader in the workspace is
/// this concatenated with `dither_core::GL_PRELUDE` and its own body, so the
/// lattice kernels have one definition per language rather than one per shader.
pub const GL_PRELUDE: &str = include_str!("noise.glsl");

// ---------------------------------------------------------------------------
// Noise: 3D value-noise fBm + 3D Worley (cellular) for craters.
// ---------------------------------------------------------------------------

// The per-axis mix constants. Odd and mutually coprime-ish, so the three
// coordinate terms decorrelate before the finalizer sees them.
const KX: u32 = 0x8da6_b343;
const KY: u32 = 0xd816_3841;
const KZ: u32 = 0xcb1a_b31f;

/// The Murmur3-style finalizer, written once over anything that can multiply,
/// xor and shift — so the scalar [`hash3`] and the four-lane [`hash3x4`] cannot
/// drift apart. Well-distributed, mean ~0.5.
trait Mix32: Copy {
    fn wmul(self, k: u32) -> Self;
    fn xor(self, o: Self) -> Self;
    fn shr(self, n: u32) -> Self;
}
impl Mix32 for u32 {
    #[inline(always)]
    fn wmul(self, k: u32) -> u32 {
        self.wrapping_mul(k)
    }
    #[inline(always)]
    fn xor(self, o: u32) -> u32 {
        self ^ o
    }
    #[inline(always)]
    fn shr(self, n: u32) -> u32 {
        self >> n
    }
}
impl Mix32 for U32x4 {
    #[inline(always)]
    fn wmul(self, k: u32) -> U32x4 {
        U32x4::wmul(self, k)
    }
    #[inline(always)]
    fn xor(self, o: U32x4) -> U32x4 {
        U32x4::xor(self, o)
    }
    #[inline(always)]
    fn shr(self, n: u32) -> U32x4 {
        U32x4::shr(self, n)
    }
}

#[inline(always)]
fn avalanche<T: Mix32>(h: T) -> T {
    let h = h.xor(h.shr(16));
    let h = h.wmul(0x7feb_352d);
    let h = h.xor(h.shr(15));
    let h = h.wmul(0x846c_a68b);
    h.xor(h.shr(16))
}

pub fn hash3(x: i32, y: i32, z: i32) -> f32 {
    let h = (x as u32).wmul(KX).xor((y as u32).wmul(KY)).xor((z as u32).wmul(KZ));
    (avalanche(h) as f32) / (u32::MAX as f32)
}

/// Four [`hash3`] evaluations at once, bit-identical to hashing each lane on its
/// own. Everything below hashes lattice corners through this.
#[inline(always)]
fn hash3x4(x: U32x4, y: U32x4, z: U32x4) -> F32x4 {
    avalanche(x.wmul(KX).xor(y.wmul(KY)).xor(z.wmul(KZ))).to_unit_f32()
}

pub fn smoother(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Held **out of line on the vector path only**, which is worth a paragraph
/// because the numbers are counter-intuitive (64² `terran` frame, V8/x86):
///
/// | value_noise              | inlined | out of line |
/// |-------------------------|---------|-------------|
/// | scalar                  | 3.46 ms | 3.60 ms     |
/// | four-lane               | 4.12 ms | 3.10 ms     |
///
/// A shader pixel evaluates this ~28 times. Inlined, the `v128` temporaries all
/// go live inside the pixel loop at once, the register allocator spills, and
/// vectorizing comes out *slower* than not bothering. Out of line the spills go
/// away and the win lands — while the scalar path, which has registers to
/// spare, would rather stay inlined. Hence the `cfg_attr` rather than a plain
/// attribute: each backend gets the inlining it actually wants.
#[cfg_attr(all(target_arch = "wasm32", target_feature = "simd128"), inline(never))]
pub fn value_noise(x: f32, y: f32, z: f32) -> f32 {
    let (xi, yi, zi) = (x.floor(), y.floor(), z.floor());
    let (xf, yf, zf) = (x - xi, y - yi, z - zi);
    let (xi, yi, zi) = (xi as i32, yi as i32, zi as i32);
    let (u, v, w) = (smoother(xf), smoother(yf), smoother(zf));

    // The eight corners, hashed as two four-lane groups laid out
    //   [ (·,0,0), (·,1,0), (·,0,1), (·,1,1) ]
    // so `lo` (x+0) and `hi` (x+1) differ only in the x term: the y/z half of
    // the mix is built once and shared, and the lerp along u then collapses the
    // pair into all four x-edge values in one vector op.
    //
    // Note the shape of the two `from_i32(..).wadd(..)` pairs: the vector part
    // is a literal, so it lowers to a single constant, and only the splat-add is
    // real work. Writing this the obvious way — `from_i32(yi, yi+1, yi, yi+1)` —
    // instead lowers to a chain of four `replace_lane`s, which measured *slower*
    // than not vectorizing at all.
    let yz = U32x4::from_i32(0, 1, 0, 1)
        .wadd(yi as u32)
        .wmul(KY)
        .xor(U32x4::from_i32(0, 0, 1, 1).wadd(zi as u32).wmul(KZ));
    let edge = |xc: i32| avalanche(U32x4::splat((xc as u32).wrapping_mul(KX)).xor(yz)).to_unit_f32();
    let (lo, hi) = (edge(xi), edge(xi + 1));
    let [x00, x10, x01, x11] = lo.add(hi.sub(lo).scale(u)).lanes();

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
/// `warp_oct == main_oct` is the undifferentiated original.
pub fn fbm_warp(x: f32, y: f32, z: f32, warp_oct: u32, main_oct: u32, w: f32) -> f32 {
    let qx = fbm(x, y, z, warp_oct);
    let qy = fbm(x + 3.1, y + 1.7, z + 5.2, warp_oct);
    let qz = fbm(x + 8.3, y + 2.8, z + 1.1, warp_oct);
    fbm(x + w * qx, y + w * qy, z + w * qz, main_oct)
}

/// The 3×3×3 neighbourhood [`worley`] searches, padded from 27 to 28 entries so
/// it divides into four-lane groups. The pad repeats the last cell, which costs
/// nothing and changes nothing: `min` is idempotent.
const CELLS: [[i32; 3]; 28] = {
    let mut c = [[0i32; 3]; 28];
    let (mut i, mut dz) = (0, -1i32);
    while dz <= 1 {
        let mut dy = -1i32;
        while dy <= 1 {
            let mut dx = -1i32;
            while dx <= 1 {
                c[i] = [dx, dy, dz];
                i += 1;
                dx += 1;
            }
            dy += 1;
        }
        dz += 1;
    }
    c[27] = c[26];
    c
};

/// 3D Worley F1: distance to nearest hashed feature point. ~[0, 1.0].
pub fn worley(x: f32, y: f32, z: f32) -> f32 {
    let (fx, fy, fz) = (x.floor() as i32, y.floor() as i32, z.floor() as i32);
    let (vx, vy, vz) = (F32x4::splat(x), F32x4::splat(y), F32x4::splat(z));
    // Squared distance all the way through, then one `sqrt` at the end. `sqrt`
    // is monotonic and correctly rounded, so min-then-sqrt returns the exact
    // same f32 as sqrt-then-min — 27 square roots collapse into 1. The 9.0
    // sentinel becomes 81.0 for the same reason (and is never selected: the
    // farthest a feature point can sit is under 4).
    let mut best = F32x4::splat(81.0);
    let mut i = 0;
    while i < CELLS.len() {
        let (a, b, c, d) = (CELLS[i], CELLS[i + 1], CELLS[i + 2], CELLS[i + 3]);
        // Same trick as `value_noise`: the neighbour offsets are literals, so
        // each of these is one constant plus one splat-add, not four lane
        // inserts. The loop has a constant trip count and unrolls, which is what
        // keeps `CELLS[i]` constant-folding.
        let ux = U32x4::from_i32(a[0], b[0], c[0], d[0]).wadd(fx as u32);
        let uy = U32x4::from_i32(a[1], b[1], c[1], d[1]).wadd(fy as u32);
        let uz = U32x4::from_i32(a[2], b[2], c[2], d[2]).wadd(fz as u32);
        // The three seeded offsets that place the feature point inside its cell.
        // Adding the constants after the `as u32` cast is the same value the
        // scalar code got from `(cx + 911) as u32` — i32 add and u32 wrapping
        // add agree on the bit pattern.
        let ox = hash3x4(ux, uy, uz);
        let oy = hash3x4(ux.wadd(911), uy.wadd(733), uz.wadd(512));
        let oz = hash3x4(ux.wadd(271), uy.wadd(619), uz.wadd(188));
        // The cell coordinate widened straight from the integer vector, so it is
        // exactly `cx as f32` with no second vector to assemble.
        let dx = ux.to_i32_f32().add(ox).sub(vx);
        let dy = uy.to_i32_f32().add(oy).sub(vy);
        let dz = uz.to_i32_f32().add(oz).sub(vz);
        best = best.min(dx.mul(dx).add(dy.mul(dy)).add(dz.mul(dz)));
        i += 4;
    }
    best.min_lane().sqrt()
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
// Drift guard
// ---------------------------------------------------------------------------
//
// The lattice kernels above are vectorized; these tests pin them to the scalar
// definitions they replaced, BIT-for-bit (not approximately — `out/` is the
// regression test and a single ULP moves pixels across the dither thresholds).
//
// Note what this does and does not cover: `cargo test` runs on the host, so it
// exercises the portable array backend of `lanes`. The wasm `simd128` backend
// is checked the same way, by rendering every planet type through both wasm
// builds and comparing the pixel bytes.
#[cfg(test)]
mod tests {
    use super::*;

    /// [`value_noise`] as it was written before vectorization.
    fn value_noise_ref(x: f32, y: f32, z: f32) -> f32 {
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

    /// [`worley`] as it was written before vectorization.
    fn worley_ref(x: f32, y: f32, z: f32) -> f32 {
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

    /// A spread of sample points: negative and positive, near lattice corners
    /// (where `floor` and the corner hashes are most fragile), and out at the
    /// ~260 magnitudes `seed_offsets` reaches.
    fn probes() -> Vec<(f32, f32, f32)> {
        let mut v = Vec::new();
        let axis = [-263.5f32, -17.0, -1.0, -0.5, 0.0, 1e-7, 0.5, 1.0, 3.25, 41.75, 259.0];
        for (i, &a) in axis.iter().enumerate() {
            for (j, &b) in axis.iter().enumerate() {
                for (k, &c) in axis.iter().enumerate() {
                    // Nudge off the exact grid on two of three axes, so the
                    // sweep covers cell interiors as well as their corners.
                    v.push((a, b + j as f32 * 0.013, c + (i + k) as f32 * 0.0071));
                }
            }
        }
        v
    }

    #[test]
    fn value_noise_matches_scalar_reference() {
        for (x, y, z) in probes() {
            let (got, want) = (value_noise(x, y, z), value_noise_ref(x, y, z));
            assert_eq!(got.to_bits(), want.to_bits(), "value_noise({x}, {y}, {z})");
        }
    }

    #[test]
    fn worley_matches_scalar_reference() {
        for (x, y, z) in probes() {
            let (got, want) = (worley(x, y, z), worley_ref(x, y, z));
            assert_eq!(got.to_bits(), want.to_bits(), "worley({x}, {y}, {z})");
        }
    }

    /// The four-lane hash must agree with the scalar one lane for lane — every
    /// lattice kernel is built on that equivalence.
    #[test]
    fn hash3x4_matches_hash3() {
        let c = [-263, -1, 0, 1, 911, 70001];
        for &x in &c {
            for &y in &c {
                for &z in &c {
                    let v = hash3x4(
                        U32x4::from_i32(x, x + 1, x + 911, x - 7),
                        U32x4::from_i32(y, y + 1, y + 733, y - 7),
                        U32x4::from_i32(z, z + 1, z + 512, z - 7),
                    )
                    .lanes();
                    let want = [
                        hash3(x, y, z),
                        hash3(x + 1, y + 1, z + 1),
                        hash3(x + 911, y + 733, z + 512),
                        hash3(x - 7, y - 7, z - 7),
                    ];
                    for l in 0..4 {
                        assert_eq!(v[l].to_bits(), want[l].to_bits(), "lane {l} at ({x},{y},{z})");
                    }
                }
            }
        }
    }
}
