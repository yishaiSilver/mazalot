//! orbit — the **eccentric Keplerian orbit** mechanic, rendered as its own type.
//!
//! Pure math, zero dependencies. Where the sibling `solar` crate draws a whole
//! system on near-circular, near-face-on squashed rings, `orbit` exists to show
//! off the *real* thing: a handful of bodies travelling on genuine **elliptical,
//! inclined** orbits around a central star, with the star sitting at a **focus**
//! of each ellipse (never the centre), and bodies that visibly **speed up near
//! perihelion** and dawdle at aphelion — Kepler's second law, equal areas swept
//! in equal times, falling out of the math for free. Same seed => the same
//! choreography, forever.
//!
//! This crate is self-contained by the workspace rule (each "type" shares no
//! code with the others — only third-party deps and the manifest). It carries
//! its own compact noise/colour/dither primitives and its own tile renderers for
//! a small star and small lit spheres.
//!
//! ## The Kepler solve (mean → eccentric → true anomaly)
//!
//! An ellipse cannot be swept at constant angular rate if the second law is to
//! hold, so we never advance the on-screen angle directly. Instead, for each
//! body (see [`Body::at`]):
//!   1. The **mean anomaly** `M = M0 + n·t` advances *linearly in time* — this is
//!      the only clock, and its uniformity is what encodes "equal areas".
//!   2. We solve Kepler's equation `M = E − e·sin(E)` for the **eccentric
//!      anomaly** `E` by a few Newton iterations (it has no closed form). This is
//!      the whole trick: `E` is *not* linear in `M`, so the body crawls through
//!      the wide arc near aphelion and races through the tight arc at perihelion.
//!   3. The orbital-plane position places the star at the **focus** at the
//!      origin: `x = a·(cos E − e)`, `y = b·sin E`, with `b = a·√(1−e²)`. At
//!      `E = 0` that is `(a(1−e), 0)` — perihelion, the closest point — exactly as
//!      it should be.
//!
//! We then rotate by the **argument of periapsis** (the ellipse's long axis
//! points a different way per body), tilt the orbital plane by the body's
//! **inclination** and roll the line of nodes on screen. Because that whole
//! chain is a *linear* map, the projection of a focus-at-origin ellipse is again
//! an ellipse with the (projected) focus at the origin — so the star stays put
//! at a focus of every orbit no matter how steeply it is tilted, and the tilt's
//! out-of-screen component doubles as a depth key so bodies pass in front of and
//! behind the star.
//!
//! Pipeline per frame (see [`OrbitSystem::render`]):
//!   1. paint the dark space backdrop + a faint static starfield,
//!   2. dot in each body's dashed elliptical orbit path (and mark perihelion),
//!   3. render the star and every body to a small RGBA tile and alpha-blend them
//!      back-to-front by depth, so the geometry reads as 3D.

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
fn ramp3(a: Rgb, b: Rgb, c: Rgb, t: f32) -> Rgb {
    if t < 0.5 {
        mix(a, b, t * 2.0)
    } else {
        mix(b, c, (t - 0.5) * 2.0)
    }
}

/// Bounded, decorrelated per-body noise offsets — keep them small so f32
/// precision holds and the noise doesn't collapse into bands.
fn seed_offsets(seed: u32) -> [f32; 3] {
    [
        hash3(seed as i32, 1, 7) * 220.0 + 4.0,
        hash3(seed as i32, 2, 7) * 220.0 + 4.0,
        hash3(seed as i32, 3, 7) * 220.0 + 4.0,
    ]
}

/// Tiny deterministic RNG for system generation (SplitMix-ish over hash3).
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
// The star at the centre — a compact emissive disc with a soft corona
// ===========================================================================

/// A small star tint for the system centre: a cool→hot photosphere pair plus a
/// corona colour. Deliberately compact (the star is a supporting actor here; the
/// orbits are the show), but enough for a bit of seed-to-seed variety.
#[derive(Clone, Copy)]
struct SunKind {
    name: &'static str,
    cool: Rgb,
    hot: Rgb,
    corona: Rgb,
    gran: f32, // granulation frequency
}

const SUNS: &[SunKind] = &[
    SunKind { name: "amber",  cool: [0.62, 0.24, 0.03], hot: [1.0, 0.96, 0.78], corona: [1.0, 0.72, 0.34], gran: 4.6 },
    SunKind { name: "ember",  cool: [0.42, 0.09, 0.02], hot: [1.0, 0.80, 0.46], corona: [1.0, 0.52, 0.22], gran: 4.0 },
    SunKind { name: "argent", cool: [0.44, 0.52, 0.80], hot: [1.0, 1.0, 1.0],   corona: [0.80, 0.90, 1.0], gran: 5.6 },
    SunKind { name: "azure",  cool: [0.10, 0.26, 0.62], hot: [0.92, 0.98, 1.0], corona: [0.62, 0.82, 1.0], gran: 4.8 },
];

/// Radius of the corona halo past the disc, in disc radii.
const CORONA_REACH: f32 = 0.8;

/// Emissive photosphere colour at a rotated surface point + limb factor `mu`.
fn sun_surface(sk: &SunKind, sx: f32, sy: f32, sz: f32, ofs: [f32; 3], t: f32, mu: f32) -> Rgb {
    let f = sk.gran;
    let (px, py, pz) = (sx + ofs[0], sy + ofs[1], sz + ofs[2]);
    // A slowly boiling cell field of hot lanes over a cooler blotchy floor.
    let cells = fbm(px * f + t * 0.4, py * f, pz * f - t * 0.3, 3);
    let blotch = fbm(px * 1.1, py * 1.1, pz * 1.1 + t * 0.2, 2);
    let heat = clamp01(0.35 + 0.9 * cells - 0.3 * blotch);
    let mut col = ramp3(sk.cool, mix(sk.cool, sk.hot, 0.6), sk.hot, heat);
    // Gentle limb darkening for a spherical read.
    let limb = 0.64 + 0.36 * mu.powf(0.45);
    col = mix(mix(col, sk.cool, 0.20 * (1.0 - mu)), col, mu.sqrt());
    [col[0] * limb, col[1] * limb, col[2] * limb]
}

// ===========================================================================
// Body type table (compact) — small lit spheres, a few surfaces for variety
// ===========================================================================

const SURF_ROCK: u8 = 0; // mottled rock / regolith
const SURF_ICE: u8 = 1; // ridged frozen crust
const SURF_GAS: u8 = 2; // latitude bands
const SURF_MOLTEN: u8 = 3; // dark rock veined with glow

/// One body archetype: a surface style + a small palette + a Lambert warmth.
#[derive(Clone, Copy)]
struct BodyKind {
    name: &'static str,
    surf: u8,
    lo: Rgb,   // shaded / low tone (also gas band low, molten rock)
    hi: Rgb,   // lit / high tone (also gas band high)
    accent: Rgb, // ice tint / molten glow / atmosphere rim
    freq: f32, // surface noise frequency
    bands: f32, // gas band count
}

const BKINDS: &[BodyKind] = &[
    BodyKind { name: "rust world",  surf: SURF_ROCK,   lo: [0.30, 0.14, 0.09], hi: [0.82, 0.52, 0.32], accent: [0.55, 0.26, 0.14], freq: 3.2, bands: 0.0 },
    BodyKind { name: "slate world", surf: SURF_ROCK,   lo: [0.18, 0.19, 0.22], hi: [0.66, 0.68, 0.72], accent: [0.30, 0.33, 0.38], freq: 3.6, bands: 0.0 },
    BodyKind { name: "verdant",     surf: SURF_ROCK,   lo: [0.08, 0.20, 0.16], hi: [0.44, 0.66, 0.38], accent: [0.24, 0.42, 0.62], freq: 2.8, bands: 0.0 },
    BodyKind { name: "rime world",  surf: SURF_ICE,    lo: [0.40, 0.52, 0.72], hi: [0.86, 0.93, 1.0],  accent: [0.55, 0.72, 0.94], freq: 3.0, bands: 0.0 },
    BodyKind { name: "banded gas",  surf: SURF_GAS,    lo: [0.52, 0.38, 0.26], hi: [0.90, 0.80, 0.60], accent: [0.42, 0.34, 0.22], freq: 3.0, bands: 9.0 },
    BodyKind { name: "azure gas",   surf: SURF_GAS,    lo: [0.16, 0.34, 0.62], hi: [0.56, 0.76, 0.94], accent: [0.30, 0.46, 0.72], freq: 3.0, bands: 8.0 },
    BodyKind { name: "molten",      surf: SURF_MOLTEN, lo: [0.14, 0.08, 0.07], hi: [1.0, 0.88, 0.34],  accent: [1.0, 0.40, 0.06], freq: 3.2, bands: 0.0 },
];

/// Number of body archetypes.
pub fn body_kind_count() -> usize {
    BKINDS.len()
}
/// Name of a body archetype (wraps out of range).
pub fn body_kind_name(i: usize) -> &'static str {
    BKINDS[i % BKINDS.len()].name
}
/// Number of star tints.
pub fn sun_kind_count() -> usize {
    SUNS.len()
}
/// Name of a star tint (wraps out of range).
pub fn sun_kind_name(i: usize) -> &'static str {
    SUNS[i % SUNS.len()].name
}

// ===========================================================================
// Orbital elements + the Kepler solve
// ===========================================================================

/// One orbiting body and its full Keplerian element set. Distances are in
/// **world units** (see [`OrbitSystem::render`] for world → screen); angles are
/// radians. The star sits at a focus of this body's ellipse, at world origin.
#[derive(Clone, Copy)]
pub struct Body {
    pub kind: usize, // index into BKINDS
    pub a: f32,      // semi-major axis, world units
    pub e: f32,      // eccentricity, 0 = circle .. <1 = ellipse
    pub omega: f32,  // argument of periapsis (rotates the ellipse in its plane)
    pub incl: f32,   // inclination — how steeply the orbit plane tilts to us
    pub node: f32,   // longitude of ascending node (rolls the tilt axis on screen)
    pub n: f32,      // mean motion (radians of mean anomaly per unit time)
    pub m0: f32,     // mean anomaly at t = 0 (phase)
    pub radius: f32, // body radius, world units
    pub spin: f32,   // axial-spin turns per unit time (self rotation)
    pub seed: u32,   // this body's surface seed
}

impl Body {
    /// Semi-minor axis `b = a·√(1−e²)`.
    #[inline]
    fn b(&self) -> f32 {
        self.a * (1.0 - self.e * self.e).max(0.0).sqrt()
    }

    /// Solve Kepler's equation `M = E − e·sin E` for the eccentric anomaly `E`
    /// at time `t`. The mean anomaly `M = m0 + n·t` advances *linearly* — that
    /// uniformity is Kepler's second law. `E` is recovered by Newton's method,
    /// which converges in a handful of steps for `e < 1`; `E = M` is a fine seed
    /// away from `e → 1`.
    fn eccentric_anomaly(&self, t: f32) -> f32 {
        let m = (self.m0 + self.n * t).rem_euclid(TAU);
        let mut e = m;
        for _ in 0..6 {
            let f = e - self.e * e.sin() - m;
            let fp = 1.0 - self.e * e.cos();
            e -= f / fp;
        }
        e
    }

    /// Project the point at eccentric anomaly `ea` to a world-screen position
    /// `(x, y)` plus a depth key `z` (positive = toward the viewer, so drawn in
    /// front of the star). Screen `y` points **down** (as in the compositor).
    ///
    /// The chain is: focus-at-origin ellipse → rotate by argument of periapsis in
    /// the orbital plane → tilt by inclination about the line of nodes → roll the
    /// node line on screen. Every step is linear, so the origin (the focus, where
    /// the star lives) maps to the origin: the star stays at a focus of the
    /// *projected* ellipse at any tilt.
    fn project(&self, ea: f32) -> (f32, f32, f32) {
        // 1. In-plane, focus at origin. Perihelion (ea = 0) -> (a(1−e), 0).
        let ox = self.a * (ea.cos() - self.e);
        let oy = self.b() * ea.sin();
        // 2. Argument of periapsis: swing the long axis around within the plane.
        let (sw, cw) = self.omega.sin_cos();
        let x1 = ox * cw - oy * sw;
        let y1 = ox * sw + oy * cw;
        // 3. Inclination: tip the plane about the line of nodes (the x axis).
        //    The in-plane y compresses by cos(i); its lost extent becomes depth.
        let (si, ci) = self.incl.sin_cos();
        let x2 = x1;
        let y2 = y1 * ci;
        let z = y1 * si; // out-of-screen component -> the depth key
        // 4. Node: roll the whole tilted ellipse on screen so orbits point every
        //    which way rather than all tilting about the same screen axis.
        let (sn, cn) = self.node.sin_cos();
        let x3 = x2 * cn - y2 * sn;
        let y3 = x2 * sn + y2 * cn;
        (x3, y3, z)
    }

    /// World-screen position + depth key of the body itself at time `t`
    /// (the Kepler solve feeds the projection).
    fn at(&self, t: f32) -> (f32, f32, f32) {
        self.project(self.eccentric_anomaly(t))
    }

    /// Farthest reach of this orbit from the star (aphelion distance ≈ a(1+e)),
    /// for framing / zoom-fit.
    fn reach(&self) -> f32 {
        self.a * (1.0 + self.e) + self.radius
    }
}

// ===========================================================================
// System generation
// ===========================================================================

/// A generated system: one central star and several bodies on eccentric,
/// inclined Keplerian orbits. Deterministic in `seed`.
pub struct OrbitSystem {
    pub seed: u32,
    pub sun_kind: usize,
    pub sun_radius: f32, // world units
    pub bodies: Vec<Body>,
}

impl OrbitSystem {
    /// Build the system for `seed` with a seed-derived body count (4..=6).
    pub fn generate(seed: u32) -> OrbitSystem {
        OrbitSystem::generate_n(seed, 0)
    }

    /// Build the system for `seed`, forcing the body count when `count > 0`
    /// (0 keeps the seed-derived 4..=6, clamped to 1..=10). The auto count is
    /// still drawn from the RNG regardless, so the shared bodies are identical
    /// whether or not the count is overridden — nudging it just adds or drops the
    /// outermost orbits instead of re-rolling the whole system.
    pub fn generate_n(seed: u32, count: u32) -> OrbitSystem {
        let mut rng = Rng::new(seed ^ 0x0_5b17);
        let sun_kind = (rng.f() * SUNS.len() as f32) as usize % SUNS.len();
        let sun_radius = rng.range(30.0, 44.0);

        let auto = 4 + (rng.f() * 3.0) as usize; // 4..=6
        let n_bodies = if count > 0 { (count as usize).clamp(1, 10) } else { auto };
        let mut bodies = Vec::with_capacity(n_bodies);

        // Semi-major axes march outward from just past the corona with growing
        // gaps, so eccentric ellipses have room to swing without overlapping.
        let mut a = sun_radius * (1.0 + CORONA_REACH) + rng.range(46.0, 66.0);
        for i in 0..n_bodies {
            // Eccentricity varied per body: some nearly round, some markedly
            // elongated. This is the headline knob the crate exists to show.
            let e = rng.range(0.05, 0.6);
            // Argument of periapsis: full spread, so every long axis points a
            // different way.
            let omega = rng.range(0.0, TAU);
            // Inclination: a WIDE spread (unlike solar's near-face-on 0.8..1.0),
            // from almost face-on to steeply edge-on, so orbits read at very
            // different angles and cross in front of / behind the star.
            let incl = rng.range(0.08, 1.30);
            let node = rng.range(0.0, TAU);
            let m0 = rng.range(0.0, TAU);
            // Mean motion by Kepler's third law: n ∝ a^(−3/2), so inner bodies
            // sweep faster. Shared sign -> the whole system revolves one way.
            let n = 3.1 * (110.0 / a).powf(1.5) * rng.range(0.9, 1.1);

            let kind = pick_kind(&mut rng, i, n_bodies);
            let is_gas = BKINDS[kind].surf == SURF_GAS;
            let radius = if is_gas { rng.range(11.0, 17.0) } else { rng.range(5.0, 10.0) };
            let spin = rng.range(0.2, 0.7) * if rng.below(0.2) { -1.0 } else { 1.0 };
            let bseed = seed.wrapping_mul(2_654_435_761).wrapping_add(i as u32 * 40_503 + 1);

            bodies.push(Body { kind, a, e, omega, incl, node, n, m0, radius, spin, seed: bseed });

            // Next axis: clear this orbit's aphelion plus a growing gap.
            a = a * (1.0 + e) + radius + rng.range(40.0, 72.0) + i as f32 * 6.0;
        }

        OrbitSystem { seed, sun_kind, sun_radius, bodies }
    }

    /// Number of orbiting bodies.
    pub fn body_count(&self) -> usize {
        self.bodies.len()
    }

    /// The archetype index of body `i` (maps to [`body_kind_name`]).
    pub fn body_kind(&self, i: usize) -> usize {
        self.bodies.get(i).map(|b| b.kind).unwrap_or(0)
    }

    /// Eccentricity of body `i` (0 if out of range) — handy for a HUD.
    pub fn body_eccentricity(&self, i: usize) -> f32 {
        self.bodies.get(i).map(|b| b.e).unwrap_or(0.0)
    }

    /// The outermost extent (world units) — the largest aphelion reach, padded —
    /// for framing / zoom-fit.
    pub fn extent(&self) -> f32 {
        self.bodies
            .iter()
            .map(|b| b.reach())
            .fold(self.sun_radius, f32::max)
            + 30.0
    }

    /// A camera that fits the whole system into a `w`×`h` viewport with margin.
    pub fn fit_camera(&self, w: u32, h: u32) -> Camera {
        let ext = self.extent();
        let halfw = w as f32 * 0.5 * 0.9;
        let halfh = h as f32 * 0.5 * 0.9;
        Camera { x: 0.0, y: 0.0, zoom: (halfw / ext).min(halfh / ext) }
    }
}

/// Pick a body archetype. Inner slots skew hot/rocky/molten, outer slots skew
/// icy/gassy, but with enough slack that layouts stay varied.
fn pick_kind(rng: &mut Rng, i: usize, n: usize) -> usize {
    let frac = i as f32 / (n as f32 - 1.0).max(1.0);
    // Inner: rock/molten. Mid: rock/verdant. Outer: ice/gas.
    let inner = [0usize, 1, 6];
    let mid = [0usize, 1, 2, 4];
    let outer = [3usize, 4, 5];
    let pool: &[usize] = if frac < 0.34 { &inner } else if frac < 0.7 { &mid } else { &outer };
    pool[(rng.f() * pool.len() as f32) as usize % pool.len()]
}

// ===========================================================================
// Body tile renderers — each fills a small RGBA tile, transparent off-body
// ===========================================================================

/// Ordered-dither offset from an 8x8 Bayer matrix, in −0.5..0.5.
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
    let levels = 24.0;
    let d = bx * 0.7 / levels;
    [
        clamp01(((o[0] + d) * levels).round() / levels),
        clamp01(((o[1] + d) * levels).round() / levels),
        clamp01(((o[2] + d) * levels).round() / levels),
    ]
}

/// A rendered body ready to blit: RGBA pixels + its tile size. Alpha is 0
/// off-body, 255 on the opaque disc, and partial in the soft corona halo.
struct Tile {
    px: Vec<u8>,
    size: u32,
}

/// Render the star to a tile of diameter ~`2·rad_px` (+corona margin).
fn render_sun_tile(sk: &SunKind, seed: u32, t: f32, rad_px: f32) -> Tile {
    let margin = rad_px * CORONA_REACH + 3.0;
    let size = (((rad_px + margin) * 2.0).ceil() as u32).max(6);
    let c = size as f32 / 2.0;
    let ofs = seed_offsets(seed);
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
                col = sun_surface(sk, nx, ny, nz, ofs, t, nz);
                a = 1.0;
            } else {
                col = [0.0, 0.0, 0.0];
                a = 0.0;
            }
            // Corona halo: a soft, shimmering falloff past the limb.
            let edge = r - 1.0;
            if edge > 0.0 && edge < CORONA_REACH {
                let theta = ny.atan2(nx);
                let flare = 0.6 + 0.5 * fbm(theta.cos() * 5.0, theta.sin() * 5.0, t * 0.6, 3);
                let fall = smoothstep(CORONA_REACH, 0.0, edge).powf(1.6);
                let glow = clamp01(fall * flare);
                let cc = [sk.corona[0] * glow, sk.corona[1] * glow, sk.corona[2] * glow];
                col = [
                    clamp01(col[0] * a + cc[0]),
                    clamp01(col[1] * a + cc[1]),
                    clamp01(col[2] * a + cc[2]),
                ];
                a = clamp01(a.max(glow));
            }

            let q = quant(col, bayer(ix, iy));
            let idx = ((iy * size + ix) * 4) as usize;
            px[idx] = (q[0] * 255.0) as u8;
            px[idx + 1] = (q[1] * 255.0) as u8;
            px[idx + 2] = (q[2] * 255.0) as u8;
            px[idx + 3] = (clamp01(a) * 255.0) as u8;
        }
    }
    Tile { px, size }
}

/// Body surface albedo at a rotated surface point (no lighting yet). Returns the
/// colour plus a self-emission term (only molten worlds glow on their own).
fn body_surface(bk: &BodyKind, sx: f32, sy: f32, sz: f32, ofs: [f32; 3], spin_t: f32) -> (Rgb, f32) {
    let (px, py, pz) = (sx + ofs[0], sy + ofs[1], sz + ofs[2]);
    match bk.surf {
        SURF_GAS => {
            // Latitude bands with a little fbm turbulence and a slow zonal drift.
            let turb = (fbm(px * 3.0, py * 3.0, pz * 3.0, 2) - 0.5) * 0.5;
            let band = 0.5 + 0.5 * ((sy + turb * 0.35) * bk.bands + spin_t * 0.2).sin();
            let mut col = mix(bk.lo, bk.hi, band);
            let fine = fbm(px * 4.0, py * 4.0, pz * 4.0, 3);
            col = mix(col, bk.hi, smoothstep(0.55, 0.82, fine) * 0.25);
            (col, 0.0)
        }
        SURF_ICE => {
            let raw = fbm(px * bk.freq, py * bk.freq, pz * bk.freq, 5);
            let ridged = 1.0 - (2.0 * raw - 1.0).abs(); // cracks/ridges
            let mut col = mix(bk.lo, bk.hi, clamp01(ridged * 1.2));
            col = mix(col, bk.accent, smoothstep(0.6, 0.95, ridged) * 0.3);
            (col, 0.0)
        }
        SURF_MOLTEN => {
            let n = fbm(px * bk.freq, py * bk.freq, pz * bk.freq, 5);
            let flow = fbm(px * 2.2 + spin_t * 0.5, py * 2.2, pz * 2.2, 3);
            let glow = clamp01(smoothstep(0.46, 0.66, n) * (0.55 + 0.9 * flow));
            let gcol = mix(bk.accent, bk.hi, clamp01(n * 1.4));
            (mix(bk.lo, gcol, glow), glow)
        }
        _ => {
            // Rock: fbm mottling between the shaded and lit tones, a touch of
            // accent in the lows (basins / seas / iron staining).
            let raw = fbm(px * bk.freq, py * bk.freq, pz * bk.freq, 6);
            let mut col = mix(bk.lo, bk.hi, clamp01((raw - 0.3) * 1.7));
            col = mix(bk.accent, col, smoothstep(0.34, 0.5, raw));
            (col, 0.0)
        }
    }
}

/// Render a body to an RGBA tile, lit from screen-space direction `light`
/// (+x right, +y up, +z toward viewer). `spin_a` is the axial rotation phase.
fn render_body_tile(bk: &BodyKind, seed: u32, spin_a: f32, spin_t: f32, light: [f32; 3], rad_px: f32) -> Tile {
    let size = (((rad_px + 1.5) * 2.0).ceil() as u32).max(6);
    let c = size as f32 / 2.0;
    let ofs = seed_offsets(seed);
    let (sina, cosa) = spin_a.sin_cos();
    let l = light;
    let mut px = vec![0u8; (size * size * 4) as usize];

    for iy in 0..size {
        for ix in 0..size {
            let nx = (ix as f32 + 0.5 - c) / rad_px;
            let ny = (c - (iy as f32 + 0.5)) / rad_px;
            let d2 = nx * nx + ny * ny;

            let mut o: Rgb = [0.0, 0.0, 0.0];
            let mut a: f32 = 0.0;

            if d2 <= 1.0 {
                let nz = (1.0 - d2).sqrt();
                // Rotate the surface point around Y by the spin so it turns.
                let sx = nx * cosa + nz * sina;
                let sy = ny;
                let sz = -nx * sina + nz * cosa;

                let (col, emis) = body_surface(bk, sx, sy, sz, ofs, spin_t);

                // Lambert against the star direction (molten worlds self-light).
                let diff = (nx * l[0] + ny * l[1] + nz * l[2]).max(0.0);
                let shade = (0.08 + 0.92 * diff).max(emis);
                o = [col[0] * shade, col[1] * shade, col[2] * shade];

                // Faint atmospheric rim on the lit limb.
                let rim = (1.0 - nz).powf(3.0) * 0.5 * (0.4 + 0.6 * diff);
                o = [
                    clamp01(o[0] + bk.accent[0] * rim),
                    clamp01(o[1] + bk.accent[1] * rim),
                    clamp01(o[2] + bk.accent[2] * rim),
                ];
                a = 1.0;

                // Crisp dark limb outline for sprite readability.
                let edge = 1.0 - 1.4 / rad_px;
                if d2 > edge * edge {
                    o = [o[0] * 0.30, o[1] * 0.30, o[2] * 0.34];
                }
            }

            let q = quant(o, bayer(ix, iy));
            let idx = ((iy * size + ix) * 4) as usize;
            px[idx] = (q[0] * 255.0) as u8;
            px[idx + 1] = (q[1] * 255.0) as u8;
            px[idx + 2] = (q[2] * 255.0) as u8;
            px[idx + 3] = (clamp01(a) * 255.0) as u8;
        }
    }
    Tile { px, size }
}

// ===========================================================================
// Scene compositor
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
    /// A default 1:1 camera centred on the star.
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

/// Alpha-blend a tile centred at screen `(sx, sy)` into the RGBA `out`,
/// nearest-neighbour scaled by `scale` (1.0 = 1:1). Only the on-screen slice of
/// the destination rectangle is touched, so cost tracks visible area.
fn blit(out: &mut [u8], w: u32, h: u32, tile: &Tile, sx: f32, sy: f32, scale: f32) {
    let dsize = (tile.size as f32 * scale).round().max(1.0) as i32;
    let x0 = (sx - dsize as f32 * 0.5).floor() as i32;
    let y0 = (sy - dsize as f32 * 0.5).floor() as i32;
    let inv = 1.0 / scale;
    let ddy0 = (-y0).max(0);
    let ddy1 = (h as i32 - y0).min(dsize);
    let ddx0 = (-x0).max(0);
    let ddx1 = (w as i32 - x0).min(dsize);
    for ddy in ddy0..ddy1 {
        let dy = y0 + ddy;
        let ty = ((ddy as f32 + 0.5) * inv) as u32;
        if ty >= tile.size {
            continue;
        }
        for ddx in ddx0..ddx1 {
            let dx = x0 + ddx;
            let tx = ((ddx as f32 + 0.5) * inv) as u32;
            if tx >= tile.size {
                continue;
            }
            let si = ((ty * tile.size + tx) * 4) as usize;
            let a = tile.px[si + 3] as u32;
            if a == 0 {
                continue;
            }
            let di = ((dy as u32 * w + dx as u32) * 4) as usize;
            if a == 255 {
                out[di] = tile.px[si];
                out[di + 1] = tile.px[si + 1];
                out[di + 2] = tile.px[si + 2];
                out[di + 3] = 255;
            } else {
                let ia = 255 - a;
                out[di] = ((tile.px[si] as u32 * a + out[di] as u32 * ia) / 255) as u8;
                out[di + 1] = ((tile.px[si + 1] as u32 * a + out[di + 1] as u32 * ia) / 255) as u8;
                out[di + 2] = ((tile.px[si + 2] as u32 * a + out[di + 2] as u32 * ia) / 255) as u8;
                out[di + 3] = 255;
            }
        }
    }
}

/// Plot one additive pixel (clamped) at integer screen `(px, py)`.
fn plot(out: &mut [u8], w: u32, h: u32, px: i32, py: i32, add: [u16; 3], cap: [u8; 3]) {
    if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
        return;
    }
    let idx = ((py as u32 * w + px as u32) * 4) as usize;
    out[idx] = (out[idx] as u16 + add[0]).min(cap[0] as u16) as u8;
    out[idx + 1] = (out[idx + 1] as u16 + add[1]).min(cap[1] as u16) as u8;
    out[idx + 2] = (out[idx + 2] as u16 + add[2]).min(cap[2] as u16) as u8;
}

/// Paint the dark space backdrop plus a faint static starfield (screen-space,
/// deterministic by seed — a quiet stage that keeps the orbits the focus).
fn paint_background(out: &mut [u8], w: u32, h: u32, seed: u32) {
    // Base: a subtle top-to-bottom navy gradient.
    for iy in 0..h {
        let v = iy as f32 / h.max(1) as f32;
        let r = (0.024 + 0.014 * v) * 255.0;
        let g = (0.021 + 0.012 * v) * 255.0;
        let b = (0.055 + 0.028 * v) * 255.0;
        for ix in 0..w {
            let d = bayer(ix, iy) * 3.0;
            let idx = ((iy * w + ix) * 4) as usize;
            out[idx] = (r + d).max(0.0) as u8;
            out[idx + 1] = (g + d).max(0.0) as u8;
            out[idx + 2] = (b + d).max(0.0) as u8;
            out[idx + 3] = 255;
        }
    }
    // Stars: one per hashed grid cell that passes the threshold.
    let si = seed as i32;
    const SP: i32 = 9; // grid spacing, px
    let cx = (w as i32 / SP) + 1;
    let cy = (h as i32 / SP) + 1;
    for gy in 0..cy {
        for gx in 0..cx {
            let hh = hash3(gx, gy, si ^ 0x2f);
            if hh < 0.86 {
                continue;
            }
            let jx = (hh * 137.0).fract();
            let jy = (hh * 71.3 + 0.37).fract();
            let px = ((gx as f32 + jx) * SP as f32) as i32;
            let py = ((gy as f32 + jy) * SP as f32) as i32;
            let s = (0.4 + 0.6 * (hh - 0.86) / 0.14) * 180.0;
            plot(out, w, h, px, py, [s as u16, s as u16, (s * 1.05) as u16], [220, 220, 235]);
        }
    }
}

/// Dot in a body's orbit path as a faint dashed, rotated, eccentric, inclined
/// ellipse, and mark its perihelion with a small brighter tick. Sampling by
/// eccentric anomaly `E` traces the *geometry* of the ellipse (not the motion),
/// so the path is drawn evenly however fast the body actually moves along it.
fn paint_orbit(out: &mut [u8], w: u32, h: u32, cam: &Camera, b: &Body) {
    let steps = 260;
    for k in 0..steps {
        // Dashed: skip a couple of samples out of every few.
        if (k / 3) % 2 == 0 {
            continue;
        }
        let ea = TAU * k as f32 / steps as f32;
        let (wx, wy, _z) = b.project(ea);
        let (sx, sy) = to_screen(wx, wy, cam, w, h);
        plot(out, w, h, sx as i32, sy as i32, [26, 30, 40], [90, 96, 120]);
    }
    // Perihelion marker: the closest point, at E = 0. A small brighter plus.
    let (wx, wy, _z) = b.project(0.0);
    let (sx, sy) = to_screen(wx, wy, cam, w, h);
    let (px, py) = (sx as i32, sy as i32);
    for d in -1..=1 {
        plot(out, w, h, px + d, py, [60, 60, 40], [180, 170, 120]);
        plot(out, w, h, px, py + d, [60, 60, 40], [180, 170, 120]);
    }
}

impl OrbitSystem {
    /// Render the whole system into `out` (RGBA, `w*h*4` bytes) at time `t`, seen
    /// through `cam` (pass `None` for an auto zoom-to-fit camera).
    ///
    /// Draw order: backdrop → orbit paths → star and bodies sorted back-to-front
    /// by depth, so a body on the far side of its orbit is occluded by the star
    /// and one on the near side passes in front of it.
    pub fn render(&self, w: u32, h: u32, cam: Option<Camera>, t: f32, out: &mut [u8]) {
        assert!(out.len() >= (w * h * 4) as usize);
        let cam = cam.unwrap_or_else(|| self.fit_camera(w, h));

        paint_background(out, w, h, self.seed);
        for b in &self.bodies {
            paint_orbit(out, w, h, &cam, b);
        }

        // Draw list of (depth, index). The star sits at depth 0 (world origin);
        // bodies sort around it by the out-of-screen component of their orbit.
        let mut order: Vec<(f32, i32)> = Vec::with_capacity(self.bodies.len() + 1);
        order.push((0.0, -1)); // star
        for (i, b) in self.bodies.iter().enumerate() {
            let (_, _, depth) = b.at(t);
            order.push((depth, i as i32));
        }
        order.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        let (suncx, suncy) = to_screen(0.0, 0.0, &cam, w, h);
        let sun = &SUNS[self.sun_kind];
        let (wf, hf) = (w as f32, h as f32);
        // A body fully outside the padded viewport is skipped (matters once a
        // tight camera is panned off the star).
        let offscreen = |bx: f32, by: f32, r: f32, pad: f32| {
            let e = r * pad;
            bx + e < 0.0 || bx - e > wf || by + e < 0.0 || by - e > hf
        };

        for (_, which) in order {
            if which < 0 {
                let rad_px = self.sun_radius * cam.zoom;
                if rad_px < 0.5 || offscreen(suncx, suncy, rad_px, 1.0 + CORONA_REACH) {
                    continue;
                }
                let rad_render = rad_px.clamp(2.0, 140.0);
                let tile = render_sun_tile(sun, self.seed, t, rad_render);
                blit(out, w, h, &tile, suncx, suncy, rad_px / rad_render);
            } else {
                let b = &self.bodies[which as usize];
                let (wx, wy, _depth) = b.at(t);
                let (sx, sy) = to_screen(wx, wy, &cam, w, h);
                let rad_px = b.radius * cam.zoom;
                if rad_px < 0.4 || offscreen(sx, sy, rad_px, 1.4) {
                    continue;
                }
                // Light from the star: direction body → star in screen space
                // (+x right, +y up — screen y is down, so flip), biased toward the
                // viewer so the terminator sits pleasingly.
                let (dx, dy) = (suncx - sx, suncy - sy);
                let lmag = (dx * dx + dy * dy).sqrt().max(1e-3);
                let (lx, ly) = (dx / lmag, -dy / lmag);
                let lz = 0.55;
                let m = (lx * lx + ly * ly + lz * lz).sqrt();
                let light = [lx / m, ly / m, lz / m];

                let bk = &BKINDS[b.kind];
                let spin_a = b.m0 + b.spin * t * TAU; // axial rotation
                let rad_render = rad_px.clamp(2.0, 120.0);
                let tile = render_body_tile(bk, b.seed, spin_a, t, light, rad_render);
                blit(out, w, h, &tile, sx, sy, rad_px / rad_render);
            }
        }
    }

    /// World position of body `i` at time `t` (for a camera that follows a body
    /// along its ellipse). Returns `(0, 0)` — the star — for an out-of-range index.
    pub fn body_world_pos(&self, i: usize, t: f32) -> (f32, f32) {
        match self.bodies.get(i) {
            Some(b) => {
                let (x, y, _) = b.at(t);
                (x, y)
            }
            None => (0.0, 0.0),
        }
    }
}

// Browser (wasm) C-ABI glue — excluded from native builds. See wasm.rs.
#[cfg(target_arch = "wasm32")]
mod wasm;
