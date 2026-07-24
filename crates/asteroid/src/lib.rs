//! asteroid — a procedural, seed-driven **asteroid belt** that slowly revolves.
//!
//! Pure math, zero dependencies. Where `solar` renders a whole system (a star
//! with planets you can drag around), `asteroid` renders one *ring of rubble*:
//! a drifting annular field of hundreds of small rocks orbiting an empty focus,
//! squashed vertically for a tilted, near-top-down read (the same `ORBIT_FLATTEN`
//! trick `solar` uses for its orbits). Same seed => the same belt, forever.
//!
//! This crate is self-contained by the workspace rule (each "type" crate shares
//! no code with the others — only third-party deps and the manifest). It carries
//! its own compact noise/color/dither primitives and its own tiny lit-sprite
//! renderer for the few big rocks, tuned to read at the handful-of-pixels scale
//! a belt view needs. The new work here is the layer on top — a Keplerian-ish
//! annulus with tasteful Kirkwood density gaps, cheap depth-shaded specks, and a
//! draggable camera.
//!
//! Pipeline per frame (see [`render_belt`]):
//!   1. paint the dark parallax starfield for the current camera,
//!   2. sort the rocks back-to-front by orbital depth,
//!   3. plot each rock — most as a 1–3px depth-lit speck (`solar`'s star
//!      cell-plot loop in spirit), a few big ones as lumpy Lambert-lit sprites
//!      with a cratered fbm surface, so the belt has texture up close.
//!
//! The heavy cost is the handful of sprites; the specks are near-free point
//! plots, so the whole scene stays cheap enough to render live while the user
//! drags — exactly the "bake-or-stay-small" guidance in the workspace README.

use std::f32::consts::TAU;

// ===========================================================================
// Noise + math primitives (this crate's own copy — shared with nobody)
// ===========================================================================

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

/// 3D Worley F1: distance to the nearest hashed feature point (~[0, 1]). Gives
/// the big rocks their crater pits.
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

/// Bounded, decorrelated per-rock noise offsets — keep them small so f32
/// precision holds and the surface noise doesn't collapse into bands.
fn seed_offsets(seed: u32) -> [f32; 3] {
    [
        hash3(seed as i32, 1, 7) * 220.0 + 4.0,
        hash3(seed as i32, 2, 7) * 220.0 + 4.0,
        hash3(seed as i32, 3, 7) * 220.0 + 4.0,
    ]
}

/// Tiny deterministic RNG for belt generation (SplitMix-ish over hash3).
struct Rng {
    seed: i32,
    ctr: i32,
}
impl Rng {
    fn new(seed: u32) -> Rng {
        Rng { seed: seed as i32, ctr: 0 }
    }
    fn f(&mut self) -> f32 {
        self.ctr = self.ctr.wrapping_add(1);
        hash3(self.seed, self.ctr, 0x9e37)
    }
    /// Uniform in [lo, hi).
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f()
    }
    fn below(&mut self, p: f32) -> bool {
        self.f() < p
    }
}

// ===========================================================================
// Ordered dither (Bayer + quantize) — the crisp pixel-art read
// ===========================================================================

/// 8x8 Bayer matrix; `bayer()` maps it into a −0.5..0.5 ordered-dither offset.
const BAYER: [u8; 64] = [
    0, 32, 8, 40, 2, 34, 10, 42, 48, 16, 56, 24, 50, 18, 58, 26, 12, 44, 4, 36, 14, 46,
    6, 38, 60, 28, 52, 20, 62, 30, 54, 22, 3, 35, 11, 43, 1, 33, 9, 41, 51, 19, 59, 27,
    49, 17, 57, 25, 15, 47, 7, 39, 13, 45, 5, 37, 63, 31, 55, 23, 61, 29, 53, 21,
];
fn bayer(x: u32, y: u32) -> f32 {
    (BAYER[((y % 8) * 8 + (x % 8)) as usize] as f32 + 0.5) / 64.0 - 0.5
}

/// Ordered-dither quantize to kill banding while staying crisp under motion.
fn quant(o: Rgb, bx: f32) -> Rgb {
    let levels = 22.0;
    let d = bx * 0.7 / levels;
    [
        clamp01(((o[0] + d) * levels).round() / levels),
        clamp01(((o[1] + d) * levels).round() / levels),
        clamp01(((o[2] + d) * levels).round() / levels),
    ]
}

// ===========================================================================
// Belt generation
// ===========================================================================

/// How much the belt is squashed vertically to fake a tilted, near-top-down
/// view (shared with `solar`'s orbits — the whole workspace tilts the same way).
const ORBIT_FLATTEN: f32 = 0.42;

/// Fixed off-screen sun direction the big rocks are lit from, in the tile's
/// screen frame (+x right, +y up, +z toward viewer). Upper-left, angled at the
/// camera so terminators fall pleasingly rather than dead edge-on.
const SUN_DIR: [f32; 3] = [-0.60, 0.55, 0.58];

/// Rock albedos: a spread of carbonaceous greys and stony red-browns. One is
/// picked per rock by seed and tinted by its brightness.
const ROCK_TINTS: &[Rgb] = &[
    [0.52, 0.50, 0.47], // grey stone
    [0.44, 0.41, 0.39], // dark grey
    [0.56, 0.46, 0.36], // tan
    [0.50, 0.38, 0.30], // red-brown
    [0.40, 0.36, 0.34], // sooty carbon
    [0.60, 0.55, 0.48], // pale dust
];

/// One rock on its orbit. Distances are in **world units** (see [`render_belt`]
/// for how world → screen works); angles are radians.
#[derive(Clone, Copy)]
pub struct Asteroid {
    pub orbit: f32,   // orbital radius, world units
    pub radius: f32,  // body radius, world units
    pub speed: f32,   // angular speed, radians per unit time
    pub phase: f32,   // angle at time 0
    pub spin: f32,    // sprite tumble turns per unit time (big rocks only)
    pub big: bool,    // rendered as a lit sprite (true) or a cheap speck (false)
    pub tint: Rgb,    // surface albedo
    pub seed: u32,    // this rock's noise seed
}

impl Asteroid {
    /// World-space position + a depth key at time `t`. `spacing` scales the
    /// orbit radius (a live UI multiplier). Depth > 0 means the rock is on the
    /// near side of the ring (drawn in front, a touch brighter and larger).
    fn at(&self, t: f32, spacing: f32) -> (f32, f32, f32) {
        let a = self.phase + self.speed * t;
        let (s, c) = a.sin_cos();
        let x = c * self.orbit * spacing;
        let y = s * self.orbit * ORBIT_FLATTEN * spacing;
        (x, y, s) // depth = sin(a): +1 at the front of the ellipse
    }
}

/// A whole generated asteroid belt: an empty focus ringed by many rocks.
/// Deterministic in `seed`. The `view` multipliers below are live, UI-tunable
/// overrides that do NOT change the belt's identity (same rocks, just rescaled)
/// — only the seed is structural.
pub struct Belt {
    pub seed: u32,
    pub inner: f32, // annulus inner radius, world units
    pub outer: f32, // annulus outer radius, world units
    pub rocks: Vec<Asteroid>,
    // --- live view multipliers (1.0 = as generated) ---
    pub spacing: f32,       // orbit-radius scale (belt size)
    pub rock_size: f32,     // body-radius scale (rock chunkiness)
    pub star_density: f32,  // background starfield density (0 = none)
    pub show_center: bool,  // draw the faint central focus marker
}

impl Belt {
    /// Build the belt for `seed` with a seed-derived rock count (~380..700).
    pub fn generate(seed: u32) -> Belt {
        Belt::generate_n(seed, 0)
    }

    /// Build the belt for `seed`, forcing the rock count when `count_override > 0`
    /// (0 keeps the seed-derived ~380..700). The auto count is still drawn from
    /// the RNG either way, so the shared rocks are identical whether or not the
    /// count is forced — nudging it just adds/removes the last-scattered rocks.
    pub fn generate_n(seed: u32, count_override: u32) -> Belt {
        let mut rng = Rng::new(seed ^ 0xA57E_0B17);
        let inner = rng.range(120.0, 175.0);
        let outer = inner + rng.range(150.0, 235.0);

        let auto = 380 + (rng.f() * 320.0) as usize; // ~380..700
        let count = if count_override > 0 {
            (count_override as usize).clamp(8, 4000)
        } else {
            auto
        };

        // Two or three Kirkwood-style resonance gaps at seed-varied radii — thin
        // low-density annuli that give the belt structure without a texture.
        let gaps = [
            (rng.range(0.18, 0.34), rng.range(0.030, 0.055)),
            (rng.range(0.44, 0.60), rng.range(0.028, 0.050)),
            (rng.range(0.70, 0.86), rng.range(0.024, 0.045)),
        ];

        // Reference radius for the Keplerian speed law (inner sweeps faster).
        let kref = inner;

        let mut rocks = Vec::with_capacity(count);
        for i in 0..count {
            // Sample an orbit radius, biased away from the gaps by rejection.
            let mut orbit = inner;
            for _ in 0..6 {
                let cand = rng.range(inner, outer);
                let rn = (cand - inner) / (outer - inner);
                orbit = cand;
                if rng.f() < belt_density(rn, &gaps) {
                    break;
                }
            }

            // A small fraction are "big" rocks rendered as lit sprites; the rest
            // are tiny lit specks.
            let big = rng.below(0.055);
            let radius = if big {
                rng.range(3.2, 6.6)
            } else {
                rng.range(0.55, 1.7)
            };

            // Keplerian-ish: inner rocks sweep faster. Direction shared so the
            // whole belt revolves the same way. A little per-rock jitter keeps
            // the ring from looking like a rigid turntable.
            let speed = 0.55 * (kref / orbit).powf(1.5) * rng.range(0.9, 1.1);
            let phase = rng.range(0.0, TAU);
            let spin = rng.range(0.2, 0.8) * if rng.below(0.5) { -1.0 } else { 1.0 };

            // Rocky albedo tinted by a per-rock brightness (carbonaceous rocks
            // are dim, stony ones catch more light).
            let base = ROCK_TINTS[(rng.f() * ROCK_TINTS.len() as f32) as usize % ROCK_TINTS.len()];
            let bright = rng.range(0.45, 1.0);
            let tint = [base[0] * bright, base[1] * bright, base[2] * bright];

            let rseed = seed.wrapping_mul(2_654_435_761).wrapping_add(i as u32 * 40_503 + 1);
            rocks.push(Asteroid { orbit, radius, speed, phase, spin, big, tint, seed: rseed });
        }

        Belt {
            seed,
            inner,
            outer,
            rocks,
            spacing: 1.0,
            rock_size: 1.0,
            star_density: 0.6,
            show_center: true,
        }
    }

    /// Apply the live view multipliers (from the web UI). Sizes/spacing are
    /// clamped away from zero; star density to a sane range.
    pub fn set_view(&mut self, spacing: f32, rock_size: f32, star_density: f32, show_center: bool) {
        self.spacing = spacing.max(0.05);
        self.rock_size = rock_size.max(0.05);
        self.star_density = star_density.clamp(0.0, 4.0);
        self.show_center = show_center;
    }

    /// Number of rocks in the belt.
    pub fn rock_count(&self) -> usize {
        self.rocks.len()
    }

    /// The outermost extent (world units) with the current view multipliers —
    /// handy for framing / zoom-fit.
    pub fn extent(&self) -> f32 {
        self.outer * self.spacing + 12.0
    }
}

/// Belt population density in [0, 1] at normalized radius `rn` (0 = inner rim,
/// 1 = outer rim): a soft-edged plateau carved by a few Gaussian resonance gaps.
fn belt_density(rn: f32, gaps: &[(f32, f32); 3]) -> f32 {
    // Fade the very rims so the ring has soft edges, not hard walls.
    let mut d = smoothstep(0.0, 0.10, rn) * smoothstep(1.0, 0.90, rn);
    // A gentle overall crest so the middle is richer than the edges.
    d *= 0.7 + 0.3 * (1.0 - (rn - 0.5).abs() * 2.0);
    // Carve the gaps.
    for &(center, width) in gaps {
        let x = (rn - center) / width;
        d *= 1.0 - 0.9 * (-x * x).exp();
    }
    clamp01(d)
}

// ===========================================================================
// Camera + world→screen
// ===========================================================================

/// Camera over the world. `x,y` is the world point shown at the viewport
/// centre; `zoom` scales world units to pixels (1.0 = 1:1).
#[derive(Clone, Copy)]
pub struct Camera {
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
}
impl Camera {
    pub fn centered() -> Camera {
        Camera { x: 0.0, y: 0.0, zoom: 1.0 }
    }
}

/// World → screen for the given viewport.
#[inline]
fn to_screen(wx: f32, wy: f32, cam: &Camera, w: u32, h: u32) -> (f32, f32) {
    (
        w as f32 * 0.5 + (wx - cam.x) * cam.zoom,
        h as f32 * 0.5 + (wy - cam.y) * cam.zoom,
    )
}

// ===========================================================================
// Background
// ===========================================================================

/// Star colour by a hash in [0,1): mostly pale/blue-white, a few warm, rare cyan.
fn star_tint(hh: f32) -> Rgb {
    if hh < 0.48 {
        [0.90, 0.93, 1.00]
    } else if hh < 0.68 {
        [0.72, 0.82, 1.00]
    } else if hh < 0.84 {
        [1.00, 0.95, 0.78]
    } else if hh < 0.95 {
        [1.00, 0.82, 0.62]
    } else {
        [0.74, 1.00, 0.96]
    }
}

/// Paint the space background: a dark navy base plus two screen-space star
/// layers that scroll on **pan** only (never on zoom — so a star can't outrun
/// the belt). `bgx`/`bgy` are the accumulated screen-space camera pan; each
/// layer is a fixed pixel grid, one 1px star per visible cell — O(cells), the
/// same cheap cell-plot loop `solar` uses for its starfield.
fn paint_background(out: &mut [u8], w: u32, h: u32, seed: u32, density: f32, bgx: f32, bgy: f32) {
    // --- base navy ---
    for iy in 0..h {
        for ix in 0..w {
            let idx = ((iy * w + ix) * 4) as usize;
            out[idx] = 8;
            out[idx + 1] = 7;
            out[idx + 2] = 17;
            out[idx + 3] = 255;
        }
    }

    let d = density.max(0.0);
    if d <= 0.001 {
        return;
    }
    // (parallax p, screen grid px, base threshold, brightness, salt)
    let layers: [(f32, f32, f32, f32, i32); 2] = [
        (0.18, 7.0, 0.82, 0.60, 0), // far — slow, dim
        (0.36, 10.0, 0.86, 0.95, 1), // near — more parallax, brighter
    ];
    let (wi, hi) = (w as i32, h as i32);
    // Salt the star hash with the belt seed so each belt gets its own sky.
    let sky = seed as i32;
    for (p, sp, base_thr, bri, salt) in layers {
        let thr = 1.0 - (1.0 - base_thr) * d;
        if thr >= 0.9999 {
            continue;
        }
        let inv = 1.0 / sp;
        let (ox, oy) = (bgx * p, bgy * p);
        let (c0x, c1x) = ((ox * inv).floor() as i32 - 1, ((ox + w as f32) * inv).floor() as i32 + 1);
        let (c0y, c1y) = ((oy * inv).floor() as i32 - 1, ((oy + h as f32) * inv).floor() as i32 + 1);
        for cy in c0y..=c1y {
            for cx in c0x..=c1x {
                let hh = hash3(cx, cy, sky.wrapping_add(17 + salt));
                if hh <= thr {
                    continue;
                }
                let jx = (hh * 137.0).fract(); // jitter across the cell, [0,1)
                let jy = (hh * 71.3 + 0.37).fract();
                let px = ((cx as f32 + jx) * sp - ox).floor() as i32;
                let py = ((cy as f32 + jy) * sp - oy).floor() as i32;
                if px < 0 || py < 0 || px >= wi || py >= hi {
                    continue;
                }
                let t = (hh - thr) / (1.0 - thr);
                let s = bri * (0.5 + 0.5 * t);
                let col = star_tint((hh * 313.0).fract());
                let idx = ((py as u32 * w + px as u32) * 4) as usize;
                out[idx] = (clamp01(out[idx] as f32 / 255.0 + s * col[0]) * 255.0) as u8;
                out[idx + 1] = (clamp01(out[idx + 1] as f32 / 255.0 + s * col[1]) * 255.0) as u8;
                out[idx + 2] = (clamp01(out[idx + 2] as f32 / 255.0 + s * col[2]) * 255.0) as u8;
            }
        }
    }
}

/// A faint central focus marker: a small additive blue-grey glow at the empty
/// point the belt orbits, so the composition has an anchor without a body.
fn paint_center(out: &mut [u8], w: u32, h: u32, cam: &Camera) {
    let (cx, cy) = to_screen(0.0, 0.0, cam, w, h);
    let reach = 7.0f32;
    let r = reach.ceil() as i32;
    let (icx, icy) = (cx as i32, cy as i32);
    for dy in -r..=r {
        for dx in -r..=r {
            let (px, py) = (icx + dx, icy + dy);
            if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
                continue;
            }
            let dist = ((dx * dx + dy * dy) as f32).sqrt();
            let g = smoothstep(reach, 0.0, dist) * 0.35;
            if g <= 0.0 {
                continue;
            }
            let idx = ((py as u32 * w + px as u32) * 4) as usize;
            out[idx] = (clamp01(out[idx] as f32 / 255.0 + g * 0.55) * 255.0) as u8;
            out[idx + 1] = (clamp01(out[idx + 1] as f32 / 255.0 + g * 0.62) * 255.0) as u8;
            out[idx + 2] = (clamp01(out[idx + 2] as f32 / 255.0 + g * 0.80) * 255.0) as u8;
        }
    }
}

// ===========================================================================
// Rock rendering
// ===========================================================================

/// Depth → brightness: rocks on the near side (depth ~ +1) catch a touch more
/// light than those on the far side (depth ~ −1). A subtle read, not a spotlight.
fn depth_shade(depth: f32) -> f32 {
    0.66 + 0.34 * (0.5 + 0.5 * depth)
}

/// Plot a tiny lit speck: a `d`×`d` block of the rock's tint, dithered. `d` is
/// derived from the on-screen radius and clamped to the 1–3px "belt dust" band.
/// Composited opaquely over the background — cheap, no per-pixel lighting.
fn plot_speck(out: &mut [u8], w: u32, h: u32, sx: f32, sy: f32, rad_px: f32, tint: Rgb, shade: f32) {
    let d = ((rad_px * 2.0).round() as i32).clamp(1, 3);
    let x0 = (sx - d as f32 * 0.5).round() as i32;
    let y0 = (sy - d as f32 * 0.5).round() as i32;
    let col = [tint[0] * shade, tint[1] * shade, tint[2] * shade];
    for oy in 0..d {
        let py = y0 + oy;
        if py < 0 || py >= h as i32 {
            continue;
        }
        for ox in 0..d {
            let px = x0 + ox;
            if px < 0 || px >= w as i32 {
                continue;
            }
            let q = quant(col, bayer(px as u32, py as u32));
            let idx = ((py as u32 * w + px as u32) * 4) as usize;
            out[idx] = (q[0] * 255.0) as u8;
            out[idx + 1] = (q[1] * 255.0) as u8;
            out[idx + 2] = (q[2] * 255.0) as u8;
            out[idx + 3] = 255;
        }
    }
}

/// Render a big rock as a lumpy Lambert-lit sprite, composited into `out` at
/// screen `(sx, sy)`. The silhouette is a disc warped by a low-frequency radial
/// noise (so no two rocks share an outline), the surface is a cratered fbm, and
/// lighting is Lambert against the fixed [`SUN_DIR`]. `spin_a` tumbles the
/// surface/silhouette so the rock turns as it drifts.
fn plot_sprite(
    out: &mut [u8],
    w: u32,
    h: u32,
    sx: f32,
    sy: f32,
    rad_px: f32,
    tint: Rgb,
    shade: f32,
    seed: u32,
    spin_a: f32,
) {
    // The silhouette warp can push the edge out to ~1.3 radii; pad the box.
    let reach = rad_px * 1.34 + 1.5;
    let x0 = (sx - reach).floor() as i32;
    let y0 = (sy - reach).floor() as i32;
    let x1 = (sx + reach).ceil() as i32;
    let y1 = (sy + reach).ceil() as i32;
    let ofs = seed_offsets(seed);
    let (sina, cosa) = spin_a.sin_cos();
    let l = SUN_DIR;

    for py in y0..=y1 {
        if py < 0 || py >= h as i32 {
            continue;
        }
        for px in x0..=x1 {
            if px < 0 || px >= w as i32 {
                continue;
            }
            let nx = (px as f32 + 0.5 - sx) / rad_px;
            let ny = (sy - (py as f32 + 0.5)) / rad_px;
            let r = (nx * nx + ny * ny).sqrt();

            // Low-freq radial warp of the unit boundary → an irregular lump.
            let theta = ny.atan2(nx) + spin_a;
            let warp = fbm(theta.cos() * 1.7 + ofs[0], theta.sin() * 1.7 + ofs[1], 3.0, 3);
            let edge = 0.78 + 0.34 * warp; // silhouette radius in [~0.78, ~1.12]
            if r > edge {
                continue;
            }

            // Fake a rounded surface inside the warped silhouette.
            let rn = (r / edge).min(1.0);
            let nz = (1.0 - rn * rn).max(0.0).sqrt();
            // Rotate the surface point about Y by the tumble so craters drift.
            let ssx = nx * cosa + nz * sina + ofs[0];
            let ssy = ny + ofs[1];
            let ssz = -nx * sina + nz * cosa + ofs[2];

            // Cratered, mottled albedo.
            let mott = fbm(ssx * 3.0, ssy * 3.0, ssz * 3.0, 4);
            let pit = smoothstep(0.16, 0.0, worley(ssx * 2.4, ssy * 2.4, ssz * 2.4));
            let mut col = [
                tint[0] * (0.72 + 0.5 * mott),
                tint[1] * (0.72 + 0.5 * mott),
                tint[2] * (0.72 + 0.5 * mott),
            ];
            col = mix(col, [col[0] * 0.5, col[1] * 0.5, col[2] * 0.52], pit * 0.7);

            // Lambert against the sun, with a little ambient fill.
            let diff = (nx * l[0] + ny * l[1] + nz * l[2]).max(0.0);
            let lit = (0.14 + 0.9 * diff) * shade;
            let mut o = [col[0] * lit, col[1] * lit, col[2] * lit];

            // Crisp darkened limb for sprite readability.
            if rn > 1.0 - 1.6 / rad_px {
                o = [o[0] * 0.34, o[1] * 0.34, o[2] * 0.38];
            }

            let q = quant(o, bayer(px as u32, py as u32));
            let idx = ((py as u32 * w + px as u32) * 4) as usize;
            out[idx] = (q[0] * 255.0) as u8;
            out[idx + 1] = (q[1] * 255.0) as u8;
            out[idx + 2] = (q[2] * 255.0) as u8;
            out[idx + 3] = 255;
        }
    }
}

// ===========================================================================
// Scene compositor
// ===========================================================================

/// Render the whole belt into `out` (RGBA, `w*h*4` bytes) at time `t`.
///
/// Draw order: starfield → central marker → rocks sorted back-to-front by
/// orbital depth, so rocks crossing the near side of the ring plot over those
/// on the far side. Big rocks come through as lit sprites; the many small ones
/// as depth-shaded specks. `t` advances the revolution + the sprite tumble.
pub fn render_belt(belt: &Belt, w: u32, h: u32, cam: &Camera, t: f32, out: &mut [u8]) {
    assert!(out.len() >= (w * h * 4) as usize);
    // The starfield is a fixed screen-space backdrop; its pan is the camera
    // displacement in screen space (cam·zoom), so pure zoom moves nothing.
    paint_background(out, w, h, belt.seed, belt.star_density, cam.x * cam.zoom, cam.y * cam.zoom);
    if belt.show_center {
        paint_center(out, w, h, cam);
    }
    draw_rocks(belt, w, h, cam, t, out);
}

/// Render ONLY the rocks onto a zeroed (transparent-black) buffer — no starfield,
/// no centre marker — so the belt can be composited over another scene (e.g. the
/// solar-system view) that already owns the background and the star at the focus.
pub fn render_belt_overlay(belt: &Belt, w: u32, h: u32, cam: &Camera, t: f32, out: &mut [u8]) {
    let len = (w * h * 4) as usize;
    assert!(out.len() >= len);
    for b in out[..len].iter_mut() {
        *b = 0;
    }
    draw_rocks(belt, w, h, cam, t, out);
}

/// The depth-sorted rock pass, shared by [`render_belt`] and
/// [`render_belt_overlay`]. Draws over whatever is already in `out`.
fn draw_rocks(belt: &Belt, w: u32, h: u32, cam: &Camera, t: f32, out: &mut [u8]) {
    let (wf, hf) = (w as f32, h as f32);
    // Depth-sort a lightweight (depth, index) list back-to-front. Hundreds of
    // entries — trivially cheap next to the per-pixel sprite work.
    let mut order: Vec<(f32, u32)> = Vec::with_capacity(belt.rocks.len());
    for (i, rk) in belt.rocks.iter().enumerate() {
        let (_, _, depth) = rk.at(t, belt.spacing);
        order.push((depth, i as u32));
    }
    order.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    for (depth, i) in order {
        let rk = &belt.rocks[i as usize];
        let (wx, wy, _) = rk.at(t, belt.spacing);
        let (sx, sy) = to_screen(wx, wy, cam, w, h);
        // Near-side rocks read a hair larger, as if a touch closer.
        let grow = 1.0 + 0.22 * depth.max(0.0);
        let rad_px = rk.radius * belt.rock_size * grow * cam.zoom;
        if rad_px < 0.25 {
            continue;
        }
        // Cull rocks fully off-screen (crucial when zoomed in).
        let e = rad_px * 1.5 + 2.0;
        if sx + e < 0.0 || sx - e > wf || sy + e < 0.0 || sy - e > hf {
            continue;
        }
        let shade = depth_shade(depth);
        if rk.big {
            let spin_a = rk.phase + rk.spin * t * TAU;
            plot_sprite(out, w, h, sx, sy, rad_px.max(2.0), rk.tint, shade, rk.seed, spin_a);
        } else {
            plot_speck(out, w, h, sx, sy, rad_px, rk.tint, shade);
        }
    }
}

// Browser (wasm) C-ABI glue — excluded from native builds. See wasm.rs.
#[cfg(target_arch = "wasm32")]
mod wasm;
