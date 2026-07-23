//! solar — a procedural, seed-driven **solar system** you can drag around.
//!
//! Pure math, zero dependencies. Where `planet` and `star` each render one body
//! filling a square, `solar` renders a *whole system* into an arbitrary
//! rectangular viewport: a central star with planets orbiting it, drawn against
//! a starfield you can pan across and zoom into. Same seed => the same system,
//! forever.
//!
//! This crate is self-contained by the workspace rule (each "type" crate shares
//! no code with the others — only third-party deps and the manifest). It carries
//! its own compact noise/color primitives and its own *tile* renderers for a
//! star and a planet: small versions of the same fake-3D sphere technique the
//! sibling crates use, tuned to read at the tens-of-pixels scale a system view
//! needs. The new work here is the layer on top — orbital layout, depth sorting
//! so planets pass in front of and behind the sun, and a draggable camera.
//!
//! Pipeline per frame (see [`render_system`]):
//!   1. paint the parallax starfield for the current camera,
//!   2. dot in each planet's orbit path,
//!   3. render every body (sun + planets) to a small RGBA tile and alpha-blend
//!      it into the scene, back-to-front, so the geometry reads as 3D.
//!
//! The heavy cost is per-body pixel work, and bodies are small, so the whole
//! scene stays cheap enough to render live while the user drags — exactly the
//! "bake-or-stay-small" guidance in the workspace README.

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
/// the sun its convection cells and, faintly, the gas bands their turbulence.
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
fn contrast(h: f32, k: f32) -> f32 {
    clamp01((h - 0.5) * k + 0.5)
}
fn ramp(stops: &[(f32, Rgb)], h: f32) -> Rgb {
    for s in stops {
        if h < s.0 {
            return s.1;
        }
    }
    stops[stops.len() - 1].1
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
// The star at the centre
// ===========================================================================

/// A small star archetype for the system centre. A compact cousin of the `star`
/// crate's table: a cool→mid→hot photosphere ramp plus a corona tint.
#[derive(Clone, Copy)]
struct SunKind {
    name: &'static str,
    cool: Rgb,
    mid: Rgb,
    hot: Rgb,
    corona: Rgb,
    gran: f32, // granulation cell frequency
}

const SUNS: &[SunKind] = &[
    SunKind { name: "yellow dwarf", cool: [0.55, 0.20, 0.02], mid: [0.99, 0.74, 0.20], hot: [1.0, 0.97, 0.82], corona: [1.0, 0.82, 0.42], gran: 5.5 },
    SunKind { name: "orange dwarf", cool: [0.35, 0.08, 0.01], mid: [0.96, 0.52, 0.14], hot: [1.0, 0.86, 0.54], corona: [1.0, 0.66, 0.30], gran: 5.0 },
    SunKind { name: "red giant",    cool: [0.26, 0.03, 0.02], mid: [0.88, 0.26, 0.09], hot: [1.0, 0.64, 0.30], corona: [1.0, 0.44, 0.20], gran: 4.0 },
    SunKind { name: "white star",   cool: [0.48, 0.56, 0.85], mid: [0.87, 0.91, 1.0],  hot: [1.0, 1.0, 1.0],   corona: [0.82, 0.90, 1.0], gran: 6.5 },
    SunKind { name: "blue giant",   cool: [0.10, 0.22, 0.60], mid: [0.47, 0.64, 1.0],  hot: [0.93, 0.98, 1.0], corona: [0.68, 0.84, 1.0], gran: 5.0 },
];

/// Radius of the corona halo past the disc, in disc radii.
const CORONA_REACH: f32 = 0.7;

/// Emissive photosphere colour at a rotated surface point + limb factor.
fn sun_surface(sk: &SunKind, sx: f32, sy: f32, sz: f32, ofs: [f32; 3], t: f32, mu: f32) -> Rgb {
    let f = sk.gran;
    let (px, py, pz) = (sx + ofs[0], sy + ofs[1], sz + ofs[2]);
    // Boil the cell field slowly over time; sample a warped worley for lanes.
    let warp = 0.5 * fbm(px * 1.6 + t * 0.4, py * 1.6, pz * 1.6 - t * 0.3, 2) - 0.25;
    let w = worley(px * f + warp, py * f + warp, pz * f);
    let blotch = fbm(px * 0.9, py * 0.9, pz * 0.9 + t * 0.2, 3);
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

// ===========================================================================
// Planet type table (compact)
// ===========================================================================

const KIND_TERRA: u8 = 0; // fbm continents + optional sea/caps
const KIND_GAS: u8 = 1; // latitude bands
const KIND_EMISSIVE: u8 = 2; // dark rock threaded with lava/glow
const KIND_ICE: u8 = 3; // ridged frozen crust

#[derive(Clone, Copy)]
struct PKind {
    name: &'static str,
    kind: u8,
    freq: f32,
    contr: f32,
    stops: &'static [(f32, Rgb)], // terrestrial / ice ramp
    band_lo: Rgb,                 // gas
    band_hi: Rgb,
    bands: f32,
    rock: Rgb, // emissive
    glow_lo: Rgb,
    glow_hi: Rgb,
    atmo: Rgb,
    caps: f32,   // polar cap coverage
    clouds: f32, // white cloud cover
    rings: bool,
    orbit_band: u8, // 0 inner (hot/rocky), 1 mid, 2 outer (cold/gas) — for placement
}

const fn pbase() -> PKind {
    PKind {
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

const TERRAN: &[(f32, Rgb)] = &[
    (0.46, [0.08, 0.16, 0.36]), (0.50, [0.13, 0.30, 0.55]), (0.52, [0.78, 0.73, 0.52]),
    (0.64, [0.28, 0.54, 0.26]), (0.78, [0.16, 0.38, 0.18]), (0.90, [0.45, 0.40, 0.34]),
    (1.01, [0.92, 0.94, 0.98]),
];
const OCEAN: &[(f32, Rgb)] = &[
    (0.58, [0.04, 0.12, 0.32]), (0.68, [0.10, 0.27, 0.51]), (0.70, [0.76, 0.70, 0.50]),
    (0.78, [0.30, 0.52, 0.30]), (1.01, [0.19, 0.42, 0.22]),
];
const DESERT: &[(f32, Rgb)] = &[
    (0.42, [0.52, 0.32, 0.19]), (0.54, [0.78, 0.55, 0.32]), (0.68, [0.87, 0.69, 0.43]),
    (0.82, [0.93, 0.82, 0.57]), (1.01, [0.72, 0.50, 0.34]),
];
const JUNGLE: &[(f32, Rgb)] = &[
    (0.44, [0.06, 0.20, 0.34]), (0.50, [0.14, 0.34, 0.44]), (0.53, [0.30, 0.42, 0.20]),
    (0.70, [0.18, 0.44, 0.18]), (0.86, [0.12, 0.32, 0.14]), (1.01, [0.50, 0.56, 0.32]),
];
const ICE: &[(f32, Rgb)] = &[
    (0.32, [0.83, 0.91, 0.99]), (0.56, [0.68, 0.80, 0.93]), (0.76, [0.50, 0.66, 0.86]),
    (1.01, [0.34, 0.51, 0.78]),
];
const BARREN: &[(f32, Rgb)] = &[
    (0.44, [0.22, 0.20, 0.22]), (0.60, [0.34, 0.32, 0.34]), (0.78, [0.50, 0.48, 0.49]),
    (1.01, [0.66, 0.64, 0.63]),
];

/// The planet archetypes. Placement leans on `orbit_band` so systems read
/// naturally (rock near the star, gas/ice far out) without being rigid.
const PKINDS: &[PKind] = &[
    PKind { name: "terran", kind: KIND_TERRA, stops: TERRAN, caps: 0.85, clouds: 0.8, atmo: [0.30, 0.45, 0.65], freq: 2.2, contr: 2.1, orbit_band: 1, ..pbase() },
    PKind { name: "ocean", kind: KIND_TERRA, stops: OCEAN, caps: 0.6, clouds: 0.7, atmo: [0.25, 0.42, 0.66], freq: 2.4, contr: 1.7, orbit_band: 1, ..pbase() },
    PKind { name: "jungle", kind: KIND_TERRA, stops: JUNGLE, caps: 0.2, clouds: 0.6, atmo: [0.28, 0.48, 0.40], freq: 2.6, contr: 1.8, orbit_band: 1, ..pbase() },
    PKind { name: "desert", kind: KIND_TERRA, stops: DESERT, caps: 0.1, clouds: 0.1, atmo: [0.38, 0.28, 0.18], freq: 2.6, contr: 1.5, orbit_band: 0, ..pbase() },
    PKind { name: "barren", kind: KIND_TERRA, stops: BARREN, caps: 0.0, clouds: 0.0, atmo: [0.0, 0.0, 0.0], freq: 3.6, contr: 1.9, orbit_band: 0, ..pbase() },
    PKind { name: "lava", kind: KIND_EMISSIVE, rock: [0.16, 0.09, 0.07], glow_lo: [1.0, 0.42, 0.06], glow_hi: [1.0, 0.92, 0.35], atmo: [0.30, 0.10, 0.05], freq: 3.0, orbit_band: 0, ..pbase() },
    PKind { name: "ice", kind: KIND_ICE, stops: ICE, caps: 0.0, atmo: [0.45, 0.60, 0.85], freq: 2.8, contr: 1.4, orbit_band: 2, ..pbase() },
    PKind { name: "gas giant", kind: KIND_GAS, band_lo: [0.55, 0.40, 0.28], band_hi: [0.88, 0.79, 0.62], bands: 11.0, atmo: [0.40, 0.34, 0.22], orbit_band: 2, ..pbase() },
    PKind { name: "ice giant", kind: KIND_GAS, band_lo: [0.20, 0.38, 0.66], band_hi: [0.55, 0.74, 0.92], bands: 9.0, atmo: [0.30, 0.45, 0.70], orbit_band: 2, ..pbase() },
    PKind { name: "ringed giant", kind: KIND_GAS, band_lo: [0.50, 0.40, 0.30], band_hi: [0.84, 0.76, 0.60], bands: 10.0, atmo: [0.36, 0.30, 0.20], rings: true, orbit_band: 2, ..pbase() },
];

/// Names of the planet archetypes, index-aligned with `PKINDS`.
pub fn planet_kind_count() -> usize {
    PKINDS.len()
}
/// Name of a planet archetype (wraps out of range).
pub fn planet_kind_name(i: usize) -> &'static str {
    PKINDS[i % PKINDS.len()].name
}
/// Number of star archetypes.
pub fn sun_kind_count() -> usize {
    SUNS.len()
}
/// Name of a star archetype (wraps out of range).
pub fn sun_kind_name(i: usize) -> &'static str {
    SUNS[i % SUNS.len()].name
}

// ===========================================================================
// System generation
// ===========================================================================

/// One planet on its orbit. All distances are in **world units** (see
/// [`render_system`] for how world → screen works); angles are radians.
#[derive(Clone, Copy)]
pub struct Planet {
    pub kind: usize,     // index into PKINDS
    pub orbit: f32,      // orbital radius, world units
    pub radius: f32,     // body radius, world units
    pub speed: f32,      // angular speed, radians per unit time
    pub phase: f32,      // angle at time 0
    pub tilt: f32,       // orbit foreshortening (0 = edge-on line, 1 = face-on circle)
    pub spin: f32,       // axial-spin turns per unit time (self rotation)
    pub seed: u32,       // this body's noise seed
}

impl Planet {
    /// World-space position + a depth key at time `t`. `spacing` scales the
    /// orbit radius (a live UI multiplier). Depth > 0 means the planet is on the
    /// near side of its orbit (drawn in front of the sun).
    fn at(&self, t: f32, spacing: f32) -> (f32, f32, f32) {
        let a = self.phase + self.speed * t;
        let (s, c) = a.sin_cos();
        let x = c * self.orbit * spacing;
        let y = s * self.orbit * ORBIT_FLATTEN * self.tilt * spacing;
        (x, y, s) // depth = sin(a): +1 at the front of the ellipse
    }
}

/// A whole generated solar system: one star and its planets. Deterministic in
/// `seed`. The `view` multipliers below are live, UI-tunable overrides that do
/// NOT change the system's identity (same worlds, just rescaled) — only the
/// seed and planet count are structural.
pub struct System {
    pub seed: u32,
    pub sun_kind: usize,
    pub sun_radius: f32, // world units
    pub planets: Vec<Planet>,
    // --- live view multipliers (1.0 = as generated) ---
    pub spacing: f32,      // orbit-radius scale (planet spacing)
    pub planet_size: f32,  // planet body-radius scale
    pub sun_size: f32,     // sun radius scale
    pub planet_pixel: f32, // planet render chunkiness (>= 1, bigger = blockier)
    pub sun_pixel: f32,    // sun render chunkiness (>= 1)
    // --- per-body detail caps (max tile radius, px) — the "how far you can zoom
    // in before it stays pixelated" floor; smaller = coarser detail sooner ---
    pub planet_detail: f32,
    pub sun_detail: f32,
}

/// How much orbits are squashed vertically to fake a tilted, near-top-down view.
const ORBIT_FLATTEN: f32 = 0.42;

impl System {
    /// Build the system for `seed` with the seed-derived planet count (4..=8).
    pub fn generate(seed: u32) -> System {
        System::generate_n(seed, 0)
    }

    /// Build the system for `seed`, forcing the planet count when
    /// `count_override > 0` (0 keeps the seed-derived 4..=8). The auto count is
    /// still drawn from the RNG either way, so the shared planets are identical
    /// whether or not the count is forced — nudging the count just adds/removes
    /// the outermost worlds instead of re-rolling the whole system.
    pub fn generate_n(seed: u32, count_override: u32) -> System {
        let mut rng = Rng::new(seed ^ 0x5013_a1);
        let sun_kind = (rng.f() * SUNS.len() as f32) as usize % SUNS.len();
        // Bigger, cooler stars get a bigger disc.
        let sun_radius = match SUNS[sun_kind].name {
            "red giant" => 62.0,
            "blue giant" => 56.0,
            "white star" => 42.0,
            "orange dwarf" => 44.0,
            _ => 48.0,
        };

        let auto = 4 + (rng.f() * 5.0) as usize; // 4..=8
        let count = if count_override > 0 { (count_override as usize).clamp(1, 16) } else { auto };
        let mut planets = Vec::with_capacity(count);
        // Orbits march outward from just past the corona with growing gaps.
        let mut orbit = sun_radius + 78.0;
        for i in 0..count {
            // Which band is this slot? Inner slots skew hot/rocky, outer cold.
            let frac = i as f32 / (count as f32 - 1.0).max(1.0);
            let want_band: u8 = if frac < 0.34 {
                0
            } else if frac < 0.7 {
                1
            } else {
                2
            };
            // Pick a type whose orbit_band matches, else anything.
            let kind = pick_kind(&mut rng, want_band);
            let pk = &PKINDS[kind];
            let is_giant = pk.kind == KIND_GAS;
            let radius = if is_giant {
                rng.range(22.0, 34.0)
            } else {
                rng.range(9.0, 17.0)
            };
            // Keplerian-ish: inner planets sweep faster. Direction shared so the
            // whole system revolves the same way.
            let speed = 0.5 * (140.0f32 / orbit).powf(1.5) * rng.range(0.85, 1.15);
            let phase = rng.range(0.0, TAU);
            let tilt = rng.range(0.8, 1.0); // near face-on, a touch of variety
            let spin = rng.range(0.15, 0.6) * if rng.below(0.15) { -1.0 } else { 1.0 };
            let bseed = seed.wrapping_mul(2_654_435_761).wrapping_add(i as u32 * 40_503 + 1);
            planets.push(Planet {
                kind,
                orbit,
                radius,
                speed,
                phase,
                tilt,
                spin,
                seed: bseed,
            });
            // Next orbit: leave room for this body + a growing gap.
            orbit += radius + rng.range(58.0, 96.0) + i as f32 * 8.0;
        }

        System {
            seed, sun_kind, sun_radius, planets,
            spacing: 1.0, planet_size: 1.0, sun_size: 1.0, planet_pixel: 1.0, sun_pixel: 1.0,
            planet_detail: 160.0, sun_detail: 110.0,
        }
    }

    /// Apply the live view multipliers (from the web UI). Sizes/spacing are
    /// clamped away from zero; pixel factors are >= 1 (1 = full detail); detail
    /// caps are clamped to a safe range (a hard ceiling keeps zoomed-in tiles
    /// from getting pathologically large).
    #[allow(clippy::too_many_arguments)]
    pub fn set_view(
        &mut self,
        spacing: f32,
        planet_size: f32,
        sun_size: f32,
        planet_pixel: f32,
        sun_pixel: f32,
        planet_detail: f32,
        sun_detail: f32,
    ) {
        self.spacing = spacing.max(0.05);
        self.planet_size = planet_size.max(0.05);
        self.sun_size = sun_size.max(0.05);
        self.planet_pixel = planet_pixel.max(1.0);
        self.sun_pixel = sun_pixel.max(1.0);
        self.planet_detail = planet_detail.clamp(6.0, 256.0);
        self.sun_detail = sun_detail.clamp(6.0, 180.0);
    }

    /// The outermost extent (world units) with the current view multipliers —
    /// handy for framing / zoom-fit.
    pub fn extent(&self) -> f32 {
        self.planets
            .last()
            .map(|p| p.orbit * self.spacing + p.radius * self.planet_size)
            .unwrap_or(self.sun_radius * self.sun_size)
            + 40.0
    }
}

fn pick_kind(rng: &mut Rng, want_band: u8) -> usize {
    // Collect indices in the wanted band; fall back to all if none.
    let mut pool = [0usize; 16];
    let mut n = 0usize;
    for (i, k) in PKINDS.iter().enumerate() {
        if k.orbit_band == want_band {
            pool[n] = i;
            n += 1;
        }
    }
    if n == 0 {
        return (rng.f() * PKINDS.len() as f32) as usize % PKINDS.len();
    }
    pool[(rng.f() * n as f32) as usize % n]
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

/// A rendered body ready to blit: RGBA pixels + its pixel radius. Alpha is 0
/// off-body, 255 on the opaque disc, and partial in soft halos (sun corona).
struct Tile {
    px: Vec<u8>,
    size: u32,
}

/// Render the star to a tile of diameter ~`2*rad_px` (+corona margin).
fn render_sun_tile(sk: &SunKind, seed: u32, t: f32, rad_px: f32) -> Tile {
    let margin = rad_px * CORONA_REACH + 3.0;
    let size = ((rad_px + margin) * 2.0).ceil() as u32;
    let size = size.max(6);
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
                // Ragged flares around the rim + a smooth radial falloff.
                let flare = 0.6 + 0.5 * fbm(theta.cos() * 5.0, theta.sin() * 5.0, t * 0.6, 3);
                let fall = smoothstep(CORONA_REACH, 0.0, edge).powf(1.6);
                let glow = clamp01(fall * flare);
                let cc = [sk.corona[0] * glow, sk.corona[1] * glow, sk.corona[2] * glow];
                // Composite corona over whatever's here (disc or empty).
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

/// Planet surface albedo at a rotated surface point (no lighting yet).
fn planet_surface(pk: &PKind, sx: f32, sy: f32, sz: f32, ofs: [f32; 3], spin_t: f32) -> (Rgb, f32) {
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
/// viewer). `spin_a` is the axial rotation phase.
fn render_planet_tile(pk: &PKind, seed: u32, spin_a: f32, spin_t: f32, light: [f32; 3], rad_px: f32) -> Tile {
    // Ring worlds need extra margin for the ring plane.
    let ring_margin = if pk.rings { rad_px * 1.4 } else { 1.5 };
    let size = ((rad_px + ring_margin) * 2.0).ceil() as u32;
    let size = size.max(6);
    let c = size as f32 / 2.0;
    let ofs = seed_offsets(seed);
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

                let (mut col, emis) = planet_surface(pk, sx, sy, sz, ofs, spin_t);

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
/// nearest-neighbour scaled by `scale` (1.0 = 1:1). `scale > 1` blows each tile
/// pixel up into a crisp `scale`×`scale` block — this is how per-body pixelation
/// is applied: a body is rendered into a small tile, then upsized with hard
/// edges, so it turns blocky without changing its on-screen size.
fn blit(out: &mut [u8], w: u32, h: u32, tile: &Tile, sx: f32, sy: f32, scale: f32) {
    let dsize = (tile.size as f32 * scale).round().max(1.0) as i32;
    let x0 = (sx - dsize as f32 * 0.5).floor() as i32;
    let y0 = (sy - dsize as f32 * 0.5).floor() as i32;
    let inv = 1.0 / scale;
    // Iterate only the on-screen slice of the (possibly huge, when zoomed in)
    // destination rectangle — clamping the loop bounds instead of testing every
    // pixel keeps blit cost proportional to visible area, not tile size.
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

/// Low-saturation nebula tints; two are picked per system by seed.
const NEB_TINTS: &[Rgb] = &[
    [0.44, 0.20, 0.60], // violet
    [0.18, 0.36, 0.70], // blue
    [0.62, 0.24, 0.42], // rose
    [0.14, 0.52, 0.52], // teal
    [0.52, 0.34, 0.22], // dusty amber
    [0.30, 0.24, 0.68], // indigo
];

/// Star colour by a hash in [0,1): mostly pale/blue-white, a few warm, rare cyan.
fn star_tint(hh: f32) -> Rgb {
    if hh < 0.46 {
        [0.92, 0.95, 1.00]
    } else if hh < 0.64 {
        [0.72, 0.83, 1.00]
    } else if hh < 0.78 {
        [1.00, 0.96, 0.78]
    } else if hh < 0.89 {
        [1.00, 0.82, 0.60]
    } else if hh < 0.96 {
        [1.00, 0.62, 0.55]
    } else {
        [0.72, 1.00, 0.95]
    }
}

/// Paint the space background: a faint colored nebula plus parallax star layers.
///
/// Everything lives in a per-layer *screen* space offset by the camera (scaled
/// by depth `p`), NOT world space — so stars stay a constant pixel size and
/// density at any zoom (no ballooning into big squares when zoomed in) while
/// still parallax-scrolling as you pan. To stay cheap AND uncluttered when
/// zoomed in, the far star layer and the whole nebula fade out (and are skipped
/// entirely) past a couple of zoom steps — you're focused on a body then anyway.
fn paint_background(out: &mut [u8], w: u32, h: u32, cam: &Camera, seed: u32) {
    let z = cam.zoom;
    let invz = 1.0 / z;
    let (cx0, cy0) = (w as f32 * 0.5, h as f32 * 0.5);
    let si = seed as i32;
    // The background is a distant field sampled in WORLD space at a
    // parallax-reduced camera position `cam·p` (p well below 1). It therefore
    // SCALES with the scene on zoom — receding (denser) as you zoom out, thinning
    // as you zoom in, so it reads as a backdrop rather than a foreground pane —
    // and PANS slower than the central star. Because it scales about the view
    // centre (not a uniform screen translation), zoom never makes it swim, and
    // each star is a single fixed pixel so world sampling can't balloon into
    // squares. Zoom also drives a fade that declutters/skips the far layer +
    // nebula when you're zoomed in on a body.
    let far_amt = 1.0 - smoothstep(3.0, 9.0, z);
    let neb_amt = 1.0 - smoothstep(2.5, 7.0, z);

    // --- nebula: baked at low res each frame (8px blocks => pixel-art clouds) ---
    const CELL: u32 = 8;
    let nw = (w + CELL - 1) / CELL;
    let nh = (h + CELL - 1) / CELL;
    let mut neb: Vec<[f32; 3]> = Vec::new();
    if neb_amt > 0.02 {
        let ta = NEB_TINTS[(hash3(si, 1, 9) * NEB_TINTS.len() as f32) as usize % NEB_TINTS.len()];
        let tb = NEB_TINTS[(hash3(si, 2, 9) * NEB_TINTS.len() as f32) as usize % NEB_TINTS.len()];
        let np = 0.07; // nebula parallax (farthest, slowest)
        let (nox, noy) = (cam.x * np + hash3(si, 5, 2) * 500.0, cam.y * np + hash3(si, 6, 2) * 500.0);
        let f = 1.0 / 460.0;
        // Clamp how far the nebula zooms out so its soft clouds never turn into
        // high-frequency blocky noise when the whole system is a speck (it just
        // stops scaling past this — stars still recede to carry the depth cue).
        let nz = invz.min(2.2);
        neb = vec![[0.0f32; 3]; (nw * nh) as usize];
        for cy in 0..nh {
            for cx in 0..nw {
                // World position of this cell on the parallax-reduced plane.
                let gx = (nox + ((cx * CELL) as f32 - cx0) * nz) * f;
                let gy = (noy + ((cy * CELL) as f32 - cy0) * nz) * f;
                let dens = smoothstep(0.50, 0.74, fbm(gx, gy, 4.2, 3)); // patchy -> not crowded
                if dens > 0.0 {
                    let n2 = fbm(gx * 1.8 + 40.0, gy * 1.8 + 7.0, 1.5, 2);
                    let col = mix(ta, tb, clamp01((n2 - 0.35) * 2.2));
                    let k = dens * neb_amt * 0.34; // faint
                    neb[(cy * nw + cx) as usize] = [col[0] * k, col[1] * k, col[2] * k];
                }
            }
        }
    }

    // --- pass 1: base navy + nebula (cheap: no per-pixel hashing) ---
    for iy in 0..h {
        let nrow = (iy / CELL) * nw;
        for ix in 0..w {
            let (mut r, mut g, mut b) = (0.031f32, 0.027, 0.068); // base navy
            if !neb.is_empty() {
                let c = neb[(nrow + ix / CELL) as usize];
                let d = bayer(ix, iy) * 0.015; // dither -> pixel-art gradient
                r += (c[0] + d).max(0.0);
                g += (c[1] + d).max(0.0);
                b += (c[2] + d).max(0.0);
            }
            let idx = ((iy * w + ix) * 4) as usize;
            out[idx] = (clamp01(r) * 255.0) as u8;
            out[idx + 1] = (clamp01(g) * 255.0) as u8;
            out[idx + 2] = (clamp01(b) * 255.0) as u8;
            out[idx + 3] = 255;
        }
    }

    // --- pass 2: stars. Each layer is a WORLD-space grid sampled at the
    // parallax-reduced camera `cam·p`. We iterate the visible cells and plot one
    // pixel per star — O(cells), not O(pixels). Because it's world space the
    // field scales with zoom (recedes/densifies as you zoom out); `p` well below
    // 1 makes it pan slower than the central star. The far layer fades on
    // zoom-in. To bound cost when zoomed far out, the grid is coarsened only once
    // a layer would exceed ~CELL_CAP cells across (stars just thin out then).
    // (parallax p, base world grid, density threshold, brightness, salt)
    const CELL_CAP: f32 = 300.0;
    let coarsen = (w.max(h) as f32 * invz) / CELL_CAP;
    let layers: [(f32, f32, f32, f32, i32); 3] = [
        (0.12, 7.0, 0.72, 0.55, 0),  // far  — slow, dim
        (0.26, 9.0, 0.75, 0.80, 1),  // mid
        (0.45, 12.0, 0.78, 1.00, 2), // near — most parallax, brightest (still < sun)
    ];
    let (wi, hi) = (w as i32, h as i32);
    for (p, g0, thr, bri, salt) in layers {
        if salt == 0 && far_amt <= 0.02 {
            continue;
        }
        let amt = if salt == 0 { far_amt } else { 1.0 };
        let g = g0.max(coarsen);
        let (cxp, cyp) = (cam.x * p, cam.y * p);
        // Visible world range on this layer's plane → cell index bounds.
        let (minx, maxx) = (cxp - cx0 * invz, cxp + cx0 * invz);
        let (miny, maxy) = (cyp - cy0 * invz, cyp + cy0 * invz);
        let (c0x, c1x) = ((minx / g).floor() as i32 - 1, (maxx / g).floor() as i32 + 1);
        let (c0y, c1y) = ((miny / g).floor() as i32 - 1, (maxy / g).floor() as i32 + 1);
        for cy in c0y..=c1y {
            for cx in c0x..=c1x {
                let hh = hash3(cx, cy, 17 + salt);
                if hh <= thr {
                    continue;
                }
                let jx = (hh * 137.0).fract(); // jitter across the cell, [0,1)
                let jy = (hh * 71.3 + 0.37).fract();
                // World position -> screen (scales with zoom, panned by cam·p).
                let px = (cx0 + ((cx as f32 + jx) * g - cxp) * z).floor() as i32;
                let py = (cy0 + ((cy as f32 + jy) * g - cyp) * z).floor() as i32;
                if px < 0 || py < 0 || px >= wi || py >= hi {
                    continue;
                }
                let t = (hh - thr) / (1.0 - thr);
                let s = bri * (0.5 + 0.5 * t) * amt;
                let col = star_tint((hh * 313.0).fract());
                let idx = ((py as u32 * w + px as u32) * 4) as usize;
                out[idx] = (clamp01(out[idx] as f32 / 255.0 + s * col[0]) * 255.0) as u8;
                out[idx + 1] = (clamp01(out[idx + 1] as f32 / 255.0 + s * col[1]) * 255.0) as u8;
                out[idx + 2] = (clamp01(out[idx + 2] as f32 / 255.0 + s * col[2]) * 255.0) as u8;
            }
        }
    }
}

/// Dot in a planet's orbit path as a faint dashed ellipse around the sun.
fn paint_orbit(out: &mut [u8], w: u32, h: u32, cam: &Camera, p: &Planet, spacing: f32) {
    let steps = 220;
    for k in 0..steps {
        // Dashed: skip every few samples.
        if (k / 3) % 2 == 0 {
            continue;
        }
        let a = TAU * k as f32 / steps as f32;
        let (s, c) = a.sin_cos();
        let wx = c * p.orbit * spacing;
        let wy = s * p.orbit * ORBIT_FLATTEN * p.tilt * spacing;
        let (sx, sy) = to_screen(wx, wy, cam, w, h);
        let (px, py) = (sx as i32, sy as i32);
        if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
            continue;
        }
        let idx = ((py as u32 * w + px as u32) * 4) as usize;
        // Additive faint blue-grey.
        out[idx] = (out[idx] as u32 + 26).min(90) as u8;
        out[idx + 1] = (out[idx + 1] as u32 + 30).min(96) as u8;
        out[idx + 2] = (out[idx + 2] as u32 + 40).min(120) as u8;
    }
}

/// Render the whole system into `out` (RGBA, `w*h*4` bytes). Three separate
/// clocks drive the animation so the web UI can pace them independently:
/// `t_orbit` advances the orbital positions, `t_spin` the planets' axial spin +
/// surface weather, and `t_sun` the star's boil/corona. (The native bin passes
/// the same value for all three.)
///
/// Draw order: starfield → orbit paths → bodies sorted back-to-front by depth,
/// so a planet on the far side of its orbit is occluded by the sun and one on
/// the near side passes in front of it.
#[allow(clippy::too_many_arguments)]
pub fn render_system(sys: &System, w: u32, h: u32, cam: &Camera, t_orbit: f32, t_spin: f32, t_sun: f32, out: &mut [u8]) {
    assert!(out.len() >= (w * h * 4) as usize);
    paint_background(out, w, h, cam, sys.seed);
    for p in &sys.planets {
        paint_orbit(out, w, h, cam, p, sys.spacing);
    }

    // Build a draw list of (depth, is_sun, planet_index). The sun sits at
    // depth 0; planets sort around it by their orbital depth.
    let mut order: Vec<(f32, i32)> = Vec::with_capacity(sys.planets.len() + 1);
    order.push((0.0, -1)); // sun
    for (i, p) in sys.planets.iter().enumerate() {
        let (_, _, depth) = p.at(t_orbit, sys.spacing);
        order.push((depth, i as i32));
    }
    order.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let (suncx, suncy) = to_screen(0.0, 0.0, cam, w, h);
    let sun = &SUNS[sys.sun_kind];
    // A body renders into a tile of at most this radius. Detail grows with zoom
    // until it hits the cap, then `blit` just upsizes the fixed-resolution tile
    // (bigger blocks, no new detail) — this is the user-set "lower bound of
    // pixelation": how far you can zoom before it stays pixelated. The buffer
    // term (0.6·maxdim) is a safety ceiling that also keeps tiles bounded.
    let buf_cap = w.max(h) as f32 * 0.6;
    let maxr = buf_cap.min(sys.planet_detail);
    let maxr_sun = buf_cap.min(sys.sun_detail);
    let (wf, hf) = (w as f32, h as f32);
    // True if a body of on-screen radius `r` centred at (bx, by), padded by
    // `pad`× for its corona/rings, lies fully outside the viewport — then we skip
    // rendering its tile entirely (crucial when zoomed in, where most bodies fly
    // off-screen but would otherwise still render full-size tiles).
    let offscreen = |bx: f32, by: f32, r: f32, pad: f32| {
        let e = r * pad;
        bx + e < 0.0 || bx - e > wf || by + e < 0.0 || by - e > hf
    };

    for (_, which) in order {
        if which < 0 {
            // The star. Per-body pixelation: render the tile smaller by
            // `sun_pixel`, then `blit` upsizes it by the same factor, so it
            // stays the same on-screen size but turns blockier.
            let rad_px = sys.sun_radius * sys.sun_size * cam.zoom;
            if rad_px < 0.5 || offscreen(suncx, suncy, rad_px, 1.0 + CORONA_REACH) {
                continue;
            }
            let rad_render = (rad_px / sys.sun_pixel).clamp(2.0, maxr_sun);
            let tile = render_sun_tile(sun, sys.seed, t_sun, rad_render);
            blit(out, w, h, &tile, suncx, suncy, rad_px / rad_render);
        } else {
            let p = &sys.planets[which as usize];
            let (wx, wy, _depth) = p.at(t_orbit, sys.spacing);
            let (sx, sy) = to_screen(wx, wy, cam, w, h);
            let rad_px = p.radius * sys.planet_size * cam.zoom;
            // Rings reach ~2 radii; pad generously so a ringed giant isn't clipped.
            if rad_px < 0.5 || offscreen(sx, sy, rad_px, 2.2) {
                continue;
            }
            // Light comes from the sun: direction from planet toward the star,
            // in screen space (+x right, +y up), with a bias toward the viewer
            // so the terminator sits pleasingly rather than dead edge-on.
            let (dx, dy) = (suncx - sx, suncy - sy);
            let lmag = (dx * dx + dy * dy).sqrt().max(1e-3);
            let (lx, ly) = (dx / lmag, -dy / lmag); // screen y is down → flip
            let lz = 0.55;
            let m = (lx * lx + ly * ly + lz * lz).sqrt();
            let light = [lx / m, ly / m, lz / m];

            let pk = &PKINDS[p.kind];
            let spin_a = p.phase + p.spin * t_spin * TAU; // axial rotation (its own clock)
            let rad_render = (rad_px / sys.planet_pixel).clamp(2.0, maxr);
            let tile = render_planet_tile(pk, p.seed, spin_a, t_spin, light, rad_render);
            blit(out, w, h, &tile, sx, sy, rad_px / rad_render);
        }
    }
}

/// World position of planet `i` at time `t` (for a camera that follows a body
/// as it orbits). Returns `(0, 0)` — the star — for an out-of-range index.
pub fn planet_world_pos(sys: &System, i: usize, t: f32) -> (f32, f32) {
    match sys.planets.get(i) {
        Some(p) => {
            let (x, y, _) = p.at(t, sys.spacing);
            (x, y)
        }
        None => (0.0, 0.0),
    }
}

/// Index of the planet whose screen position is nearest the viewport centre at
/// time `t` (for a "now viewing…" HUD), or `-1` if none is reasonably close.
pub fn planet_nearest_center(sys: &System, w: u32, h: u32, cam: &Camera, t: f32) -> i32 {
    let (ccx, ccy) = (w as f32 * 0.5, h as f32 * 0.5);
    let mut best = -1i32;
    let mut best_d = f32::MAX;
    for (i, p) in sys.planets.iter().enumerate() {
        let (wx, wy, _) = p.at(t, sys.spacing);
        let (sx, sy) = to_screen(wx, wy, cam, w, h);
        let d = (sx - ccx).powi(2) + (sy - ccy).powi(2);
        // Only count it if the centre is within ~2.5 body radii on screen.
        let reach = (p.radius * sys.planet_size * cam.zoom * 2.5 + 24.0).powi(2);
        if d < best_d && d < reach {
            best_d = d;
            best = i as i32;
        }
    }
    best
}

// Browser (wasm) C-ABI glue — excluded from native builds. See wasm.rs.
#[cfg(target_arch = "wasm32")]
mod wasm;
