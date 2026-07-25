//! moon — a procedural, seed-driven **planet with its own moons**.
//!
//! Pure math, zero dependencies. Where `solar` renders a whole star system into
//! a draggable viewport, `moon` scopes that same depth-sorted compositor down to
//! a single stage: one lit parent planet at the centre and 2–5 satellites
//! orbiting it, each correctly passing IN FRONT OF and BEHIND the parent body as
//! it goes round. Same seed => the same planet + moons, forever.
//!
//! This crate is self-contained by the workspace rule (each "type" crate shares
//! no code with the others — only third-party deps and the manifest). It carries
//! its own compact noise/color/dither primitives and its own fake-3D sphere
//! *tile* renderers: a compact cousin of solar's planet tile for the parent, and
//! a cratered-rock tile for the moons, tuned to read at the tens-of-pixels scale.
//! The new work here over a lone body is the layer on top — orbital layout for
//! the satellites and the depth sort that makes a moon on the far side of its
//! orbit disappear behind the planet while one on the near side draws over it.
//!
//! Pipeline per frame (see [`MoonSystem::render`]):
//!   1. paint the dark space backdrop + a faint static starfield,
//!   2. dot in each moon's orbit path as a dashed ellipse,
//!   3. render the parent + every moon to a small RGBA tile and alpha-blend it
//!      into the scene, back-to-front by a `sin(angle)` depth key — exactly the
//!      trick `solar`'s `Planet::at` uses — so the geometry reads as 3D.
//!
//! Lighting is a single fixed off-screen sun: a directional Lambert term shared
//! by the parent and all moons (the sun is treated as infinitely far, so there's
//! no per-body light direction to chase). Moons cast NO shadows — they only
//! occlude by depth — which keeps the whole scene cheap enough to render live.

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

/// 3D Worley F1: distance to the nearest hashed feature point (~[0, 1]). Used
/// here to scatter the moons' impact craters — each feature point becomes a pit
/// with a bright rim.
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
// Parent planet type table (compact)
// ===========================================================================

const PK_TERRA: u8 = 0; // fbm continents + sea/caps/clouds
const PK_GAS: u8 = 1; // latitude bands
const PK_BARREN: u8 = 2; // dry cratered/rocky world

/// A parent-planet archetype: the body the moons orbit. A compact cousin of
/// solar's planet table with just the handful of looks a single hero body needs.
#[derive(Clone, Copy)]
struct ParentKind {
    name: &'static str,
    kind: u8,
    freq: f32,
    contr: f32,
    stops: &'static [(f32, Rgb)], // terran / barren surface ramp
    band_lo: Rgb,                 // gas
    band_hi: Rgb,
    bands: f32,
    atmo: Rgb,   // limb-glow tint ([0,0,0] = airless)
    caps: f32,   // polar cap coverage
    clouds: f32, // white cloud cover
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
const BARREN: &[(f32, Rgb)] = &[
    (0.44, [0.22, 0.20, 0.22]), (0.60, [0.34, 0.32, 0.34]), (0.78, [0.50, 0.48, 0.49]),
    (1.01, [0.66, 0.64, 0.63]),
];

/// The parent archetypes. One is chosen per system by seed.
const PARENTS: &[ParentKind] = &[
    ParentKind { name: "terran", kind: PK_TERRA, freq: 2.2, contr: 2.1, stops: TERRAN, band_lo: [0.0; 3], band_hi: [0.0; 3], bands: 0.0, atmo: [0.30, 0.45, 0.65], caps: 0.85, clouds: 0.8 },
    ParentKind { name: "ocean",  kind: PK_TERRA, freq: 2.4, contr: 1.7, stops: OCEAN,  band_lo: [0.0; 3], band_hi: [0.0; 3], bands: 0.0, atmo: [0.25, 0.42, 0.66], caps: 0.6,  clouds: 0.7 },
    ParentKind { name: "barren", kind: PK_BARREN, freq: 3.4, contr: 1.9, stops: BARREN, band_lo: [0.0; 3], band_hi: [0.0; 3], bands: 0.0, atmo: [0.0; 3],          caps: 0.0,  clouds: 0.0 },
    ParentKind { name: "gas giant", kind: PK_GAS, freq: 4.0, contr: 1.0, stops: &[], band_lo: [0.55, 0.40, 0.28], band_hi: [0.88, 0.79, 0.62], bands: 11.0, atmo: [0.40, 0.34, 0.22], caps: 0.0, clouds: 0.0 },
    ParentKind { name: "ice giant", kind: PK_GAS, freq: 4.0, contr: 1.0, stops: &[], band_lo: [0.20, 0.38, 0.66], band_hi: [0.55, 0.74, 0.92], bands: 9.0,  atmo: [0.30, 0.45, 0.70], caps: 0.0, clouds: 0.0 },
];

/// Number of parent-planet archetypes.
pub fn parent_kind_count() -> usize {
    PARENTS.len()
}
/// Name of a parent archetype (wraps out of range).
pub fn parent_kind_name(i: usize) -> &'static str {
    PARENTS[i % PARENTS.len()].name
}

// ===========================================================================
// Moon type table (compact)
// ===========================================================================

/// A moon archetype: a small airless body. `lo`/`hi` are the low/high-albedo
/// surface colours (dark maria vs. bright highlands); `tint` shifts the whole
/// body (icy blue-white, rusty ochre, sooty carbon); `craters` weights how
/// pocked it reads.
#[derive(Clone, Copy)]
struct MoonKind {
    name: &'static str,
    lo: Rgb,
    hi: Rgb,
    freq: f32,
    craters: f32,
}

/// The moon archetypes. Picked per moon with a little variety so a family of
/// satellites doesn't read as identical grey pebbles.
const MOONKINDS: &[MoonKind] = &[
    MoonKind { name: "grey rock", lo: [0.20, 0.20, 0.22], hi: [0.66, 0.65, 0.64], freq: 3.2, craters: 1.0 },
    MoonKind { name: "pale dust", lo: [0.40, 0.38, 0.34], hi: [0.86, 0.84, 0.78], freq: 3.0, craters: 0.8 },
    MoonKind { name: "icy",       lo: [0.42, 0.52, 0.66], hi: [0.86, 0.93, 1.00], freq: 2.6, craters: 0.6 },
    MoonKind { name: "rusty",     lo: [0.30, 0.15, 0.09], hi: [0.74, 0.46, 0.28], freq: 3.4, craters: 0.9 },
    MoonKind { name: "carbon",    lo: [0.10, 0.10, 0.12], hi: [0.34, 0.33, 0.36], freq: 3.6, craters: 1.1 },
];

/// Number of moon archetypes.
pub fn moon_kind_count() -> usize {
    MOONKINDS.len()
}
/// Name of a moon archetype (wraps out of range).
pub fn moon_kind_name(i: usize) -> &'static str {
    MOONKINDS[i % MOONKINDS.len()].name
}

// ===========================================================================
// System generation
// ===========================================================================

/// How much orbits are squashed vertically to fake a tilted, near-top-down view
/// (shared with solar's ORBIT_FLATTEN idea).
const ORBIT_FLATTEN: f32 = 0.42;

/// The fixed off-screen sun direction, in the tile's screen frame (+x right,
/// +y up, +z toward viewer). Shared by the parent and every moon: the sun is
/// treated as infinitely far, so it's a pure directional Lambert light.
const LIGHT_DIR: [f32; 3] = {
    // Pre-normalised [-0.55, 0.42, 0.72] (upper-left, biased toward the viewer).
    let (x, y, z) = (-0.55f32, 0.42f32, 0.72f32);
    let inv = 1.0 / 0.998_649; // sqrt(0.55^2 + 0.42^2 + 0.72^2)
    [x * inv, y * inv, z * inv]
};

/// One satellite on its orbit around the parent. Distances are world units (see
/// [`MoonSystem::render`] for world → screen); angles are radians.
#[derive(Clone, Copy)]
pub struct Moon {
    pub kind: usize,  // index into MOONKINDS
    pub orbit: f32,   // orbital radius, world units
    pub radius: f32,  // body radius, world units
    pub speed: f32,   // angular speed, radians per unit time (inner = faster)
    pub phase: f32,   // angle at time 0
    pub tilt: f32,    // orbit foreshortening (0 = edge-on line, 1 = face-on circle)
    pub spin: f32,    // axial-spin turns per unit time (self rotation)
    pub seed: u32,    // this moon's noise seed
}

impl Moon {
    /// World-space position + a depth key at time `t`. Depth > 0 means the moon
    /// is on the near side of its orbit (drawn in front of the parent); depth < 0
    /// puts it behind, where the parent's disc occludes it.
    fn at(&self, t: f32) -> (f32, f32, f32) {
        let a = self.phase + self.speed * t;
        let (s, c) = a.sin_cos();
        let x = c * self.orbit;
        let y = s * self.orbit * ORBIT_FLATTEN * self.tilt;
        (x, y, s) // depth = sin(a): +1 at the front of the ellipse
    }
}

/// A generated planet-with-moons. Deterministic in `seed`: the parent archetype,
/// its radius and spin, and the full moon list are all derived from it, so the
/// same seed reproduces the same scene forever.
pub struct MoonSystem {
    pub seed: u32,
    pub parent_kind: usize,
    pub parent_radius: f32, // world units
    pub parent_spin: f32,   // parent axial-spin turns per unit time
    pub moons: Vec<Moon>,
    pub orbit_width: f32,   // dashed orbit line thickness, px (1..=6)
}

impl MoonSystem {
    /// Build the planet + moons for `seed` with a seed-derived moon count (2..=5).
    pub fn generate(seed: u32) -> MoonSystem {
        MoonSystem::generate_n(seed, 0)
    }

    /// Build for `seed`, forcing the moon count when `count_override > 0`
    /// (0 keeps the seed-derived 2..=5). The auto count is still drawn from the
    /// RNG either way, so the shared moons are identical whether or not the count
    /// is forced — nudging it just adds/removes the outermost satellites instead
    /// of re-rolling the whole family.
    pub fn generate_n(seed: u32, count_override: u32) -> MoonSystem {
        let mut rng = Rng::new(seed ^ 0x3a10_be);
        let parent_kind = (rng.f() * PARENTS.len() as f32) as usize % PARENTS.len();
        let pk = &PARENTS[parent_kind];
        // Gas/ice giants are bigger discs than rocky/terran parents.
        let parent_radius = if pk.kind == PK_GAS {
            rng.range(52.0, 62.0)
        } else {
            rng.range(40.0, 50.0)
        };
        let parent_spin = rng.range(0.10, 0.30) * if rng.below(0.15) { -1.0 } else { 1.0 };

        let auto = 2 + (rng.f() * 4.0) as usize; // 2..=5
        let count = if count_override > 0 { (count_override as usize).clamp(1, 8) } else { auto };
        let mut moons = Vec::with_capacity(count);
        // Orbits march outward from just past the parent's limb with growing gaps.
        let mut orbit = parent_radius + rng.range(26.0, 40.0);
        for i in 0..count {
            let kind = (rng.f() * MOONKINDS.len() as f32) as usize % MOONKINDS.len();
            let radius = rng.range(5.0, 11.0);
            // Keplerian-ish: inner moons sweep faster. Shared sign so the whole
            // family revolves the same way (a rare retrograde outlier aside).
            let dir = if rng.below(0.12) { -1.0 } else { 1.0 };
            let speed = 0.9 * (70.0f32 / orbit).powf(1.5) * rng.range(0.85, 1.15) * dir;
            let phase = rng.range(0.0, TAU);
            let tilt = rng.range(0.72, 1.0); // near face-on, a touch of variety
            let spin = rng.range(0.2, 0.7) * if rng.below(0.2) { -1.0 } else { 1.0 };
            let mseed = seed.wrapping_mul(2_654_435_761).wrapping_add(i as u32 * 40_503 + 1);
            moons.push(Moon { kind, orbit, radius, speed, phase, tilt, spin, seed: mseed });
            // Next orbit: leave room for this body + a growing gap.
            orbit += radius + rng.range(24.0, 40.0) + i as f32 * 6.0;
        }

        MoonSystem { seed, parent_kind, parent_radius, parent_spin, moons, orbit_width: 1.0 }
    }

    /// Set the dashed orbit-line thickness in pixels, clamped to 1..=6 (1 =
    /// today's single-pixel look).
    pub fn set_orbit_width(&mut self, px: f32) {
        self.orbit_width = px.clamp(1.0, 6.0);
    }

    /// The outermost extent (world units) — handy for framing / zoom-to-fit.
    pub fn extent(&self) -> f32 {
        self.moons
            .last()
            .map(|m| m.orbit + m.radius)
            .unwrap_or(self.parent_radius)
            + 20.0
    }

    /// Render the whole scene into `out` (RGBA, `w*h*4` bytes) at time `t`.
    ///
    /// Draw order: backdrop → orbit paths → bodies sorted back-to-front by depth,
    /// so a moon on the far side of its orbit is occluded by the parent and one on
    /// the near side passes in front of it. One clock `t` drives orbital motion,
    /// axial spin, and surface drift alike (the native bin and simple wasm callers
    /// pass a single value; that's all a self-contained scene needs).
    pub fn render(&self, w: u32, h: u32, cam: &Camera, t: f32, out: &mut [u8]) {
        assert!(out.len() >= (w * h * 4) as usize);
        paint_background(out, w, h, self.seed);
        for m in &self.moons {
            paint_orbit(out, w, h, cam, m, self.orbit_width);
        }

        // Build a draw list of (depth, index). The parent sits at depth 0; the
        // moons sort around it by their orbital depth (index -1 == parent).
        let mut order: Vec<(f32, i32)> = Vec::with_capacity(self.moons.len() + 1);
        order.push((0.0, -1));
        for (i, m) in self.moons.iter().enumerate() {
            let (_, _, depth) = m.at(t);
            order.push((depth, i as i32));
        }
        order.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

        // A body renders into a tile of at most this radius; past it, `blit` just
        // upsizes the fixed-resolution tile (bigger blocks, no new detail). The
        // buffer term keeps tiles bounded when zoomed way in.
        let buf_cap = w.max(h) as f32 * 0.6;
        let (wf, hf) = (w as f32, h as f32);
        let offscreen = |bx: f32, by: f32, r: f32, pad: f32| {
            let e = r * pad;
            bx + e < 0.0 || bx - e > wf || by + e < 0.0 || by - e > hf
        };

        let (pcx, pcy) = to_screen(0.0, 0.0, cam, w, h);
        for (_, which) in order {
            if which < 0 {
                // The parent planet, at the world origin.
                let rad_px = self.parent_radius * cam.zoom;
                if rad_px < 0.5 || offscreen(pcx, pcy, rad_px, 1.3) {
                    continue;
                }
                let rad_render = rad_px.clamp(2.0, buf_cap.min(200.0));
                let pk = &PARENTS[self.parent_kind];
                let spin_a = self.parent_spin * t * TAU;
                let tile = render_parent_tile(pk, self.seed, spin_a, t, rad_render);
                blit(out, w, h, &tile, pcx, pcy, rad_px / rad_render);
            } else {
                let m = &self.moons[which as usize];
                let (wx, wy, _) = m.at(t);
                let (sx, sy) = to_screen(wx, wy, cam, w, h);
                let rad_px = m.radius * cam.zoom;
                if rad_px < 0.5 || offscreen(sx, sy, rad_px, 1.5) {
                    continue;
                }
                let rad_render = rad_px.clamp(2.0, buf_cap.min(120.0));
                let mk = &MOONKINDS[m.kind];
                let spin_a = m.phase + m.spin * t * TAU;
                let tile = render_moon_tile(mk, m.seed, spin_a, t, rad_render);
                blit(out, w, h, &tile, sx, sy, rad_px / rad_render);
            }
        }
    }

    /// Number of moons — the small accessor the wasm/bin faces need.
    pub fn moon_count(&self) -> usize {
        self.moons.len()
    }
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

/// A rendered body ready to blit: RGBA pixels + its pixel diameter. Alpha is 0
/// off-body and 255 on the opaque disc.
struct Tile {
    px: Vec<u8>,
    size: u32,
}

/// Parent surface albedo at a rotated surface point (no lighting yet).
fn parent_surface(pk: &ParentKind, sx: f32, sy: f32, sz: f32, ofs: [f32; 3], t: f32) -> Rgb {
    let (px, py, pz) = (sx + ofs[0], sy + ofs[1], sz + ofs[2]);
    match pk.kind {
        PK_GAS => {
            // Latitude bands with a little worley turbulence + slow zonal drift.
            let turb = (worley(px * 3.0, py * 3.0, pz * 3.0) - 0.5) * 0.5;
            let lat = sy + turb * 0.4;
            let band = 0.5 + 0.5 * (lat * pk.bands + t * 0.2).sin();
            let mut col = mix(pk.band_lo, pk.band_hi, band);
            let fine = fbm(px * 4.0, py * 4.0, pz * 4.0, 3);
            col = mix(col, pk.band_hi, smoothstep(0.55, 0.82, fine) * 0.3);
            col
        }
        _ => {
            // Terran / barren: fbm continents, sea level built into the ramp, caps.
            let raw = fbm(px * pk.freq, py * pk.freq, pz * pk.freq, 6);
            let h = contrast(raw, pk.contr);
            let mut col = ramp(pk.stops, h);
            let cap = smoothstep(0.72, 0.9, sy.abs()) * pk.caps;
            col = mix(col, [0.92, 0.95, 1.0], cap);
            col
        }
    }
}

/// Render the parent planet to a lit RGBA tile of diameter ~`2*rad_px`. `spin_a`
/// is the axial rotation phase; `t` drifts weather/bands.
fn render_parent_tile(pk: &ParentKind, seed: u32, spin_a: f32, t: f32, rad_px: f32) -> Tile {
    let size = ((rad_px + 1.5) * 2.0).ceil() as u32;
    let size = size.max(6);
    let c = size as f32 / 2.0;
    let ofs = seed_offsets(seed);
    let (sina, cosa) = spin_a.sin_cos();
    let has_atmo = pk.atmo != [0.0, 0.0, 0.0];
    let l = LIGHT_DIR;
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

                let mut col = parent_surface(pk, sx, sy, sz, ofs, t);

                if pk.clouds > 0.0 {
                    let (cs, cc) = (spin_a * 1.4).sin_cos();
                    let cx3 = nx * cc + nz * cs + ofs[0];
                    let cz3 = -nx * cs + nz * cc + ofs[2];
                    let cloud = fbm(cx3 * 2.8, ny * 2.8 + ofs[1], cz3 * 2.8 + t * 0.1, 4);
                    col = mix(col, [1.0, 1.0, 1.0], smoothstep(0.54, 0.72, cloud) * pk.clouds);
                }

                // Directional Lambert against the fixed sun.
                let diff = (nx * l[0] + ny * l[1] + nz * l[2]).max(0.0);
                let shade = 0.08 + 0.92 * diff;
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

/// Moon surface albedo at a rotated surface point (no lighting yet): a grey/tinted
/// highlands-vs-maria base threaded with impact craters (dark pit + bright rim).
fn moon_surface(mk: &MoonKind, sx: f32, sy: f32, sz: f32, ofs: [f32; 3]) -> Rgb {
    let (px, py, pz) = (sx + ofs[0], sy + ofs[1], sz + ofs[2]);
    // Base regolith: fbm highlands over dark maria.
    let raw = fbm(px * mk.freq, py * mk.freq, pz * mk.freq, 5);
    let h = contrast(raw, 1.5);
    let mut col = mix(mk.lo, mk.hi, h);
    // Broad dark maria patches from a low-frequency threshold.
    let maria = smoothstep(0.42, 0.30, fbm(px * 1.2, py * 1.2, pz * 1.2, 3));
    col = mix(col, mk.lo, maria * 0.6);
    // Impact craters: worley feature points become pits ringed by bright rims.
    let cf = mk.freq * 1.6;
    let cw = worley(px * cf, py * cf, pz * cf);
    let pit = smoothstep(0.16, 0.02, cw); // dark central pit near a feature point
    let rim = smoothstep(0.14, 0.22, cw) * smoothstep(0.34, 0.22, cw); // bright ring
    col = mix(col, [col[0] * 0.5, col[1] * 0.5, col[2] * 0.52], pit * mk.craters);
    col = mix(col, mk.hi, rim * mk.craters * 0.5);
    col
}

/// Render a moon to a lit RGBA tile. Airless: a pure Lambert term against the
/// fixed sun, no atmosphere, no shadows cast — depth sorting alone handles
/// occlusion by the parent.
fn render_moon_tile(mk: &MoonKind, seed: u32, spin_a: f32, _t: f32, rad_px: f32) -> Tile {
    let size = ((rad_px + 1.5) * 2.0).ceil() as u32;
    let size = size.max(6);
    let c = size as f32 / 2.0;
    let ofs = seed_offsets(seed);
    let (sina, cosa) = spin_a.sin_cos();
    let l = LIGHT_DIR;
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
                let sx = nx * cosa + nz * sina;
                let sy = ny;
                let sz = -nx * sina + nz * cosa;

                let col = moon_surface(mk, sx, sy, sz, ofs);

                // Directional Lambert. Airless bodies fall off hard toward the
                // terminator — a small ambient term keeps the dark side readable.
                let diff = (nx * l[0] + ny * l[1] + nz * l[2]).max(0.0);
                let shade = 0.06 + 0.94 * diff;
                o = [col[0] * shade, col[1] * shade, col[2] * shade];
                a = 1.0;

                // Crisp dark limb outline for sprite readability.
                let edge = 1.0 - 1.4 / rad_px;
                if d2 > edge * edge {
                    o = [o[0] * 0.30, o[1] * 0.30, o[2] * 0.32];
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

/// Camera over the world. `x,y` is the world point shown at the viewport centre;
/// `zoom` scales world units to pixels (1.0 = 1:1).
#[derive(Clone, Copy)]
pub struct Camera {
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
}
impl Camera {
    /// A camera centred on the parent planet at 1:1 zoom.
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

/// Star colour by a hash in [0,1): mostly pale/blue-white, a few warm.
fn star_tint(hh: f32) -> Rgb {
    if hh < 0.5 {
        [0.92, 0.95, 1.00]
    } else if hh < 0.72 {
        [0.72, 0.83, 1.00]
    } else if hh < 0.9 {
        [1.00, 0.96, 0.78]
    } else {
        [1.00, 0.82, 0.60]
    }
}

/// Paint the dark space backdrop plus a faint, fixed starfield. The field is a
/// static screen-space grid keyed by the system seed (this scene doesn't pan),
/// plotted one 1px star per hit cell — O(cells), cheap.
fn paint_background(out: &mut [u8], w: u32, h: u32, seed: u32) {
    // Base navy fill with an ordered-dither so the flat field still reads as
    // pixel-art rather than a dead block.
    for iy in 0..h {
        for ix in 0..w {
            let d = bayer(ix, iy) * 0.010;
            let idx = ((iy * w + ix) * 4) as usize;
            out[idx] = (clamp01(0.028 + d) * 255.0) as u8;
            out[idx + 1] = (clamp01(0.026 + d) * 255.0) as u8;
            out[idx + 2] = (clamp01(0.060 + d) * 255.0) as u8;
            out[idx + 3] = 255;
        }
    }

    // Starfield: a fixed grid; ~1 in 7 cells lights up.
    let salt = seed as i32 ^ 0x51ed;
    let sp = 9.0f32; // grid spacing, px
    let inv = 1.0 / sp;
    let (wi, hi) = (w as i32, h as i32);
    let (c1x, c1y) = ((w as f32 * inv) as i32 + 1, (h as f32 * inv) as i32 + 1);
    for cy in 0..=c1y {
        for cx in 0..=c1x {
            let hh = hash3(cx, cy, salt);
            if hh <= 0.86 {
                continue;
            }
            let jx = (hh * 137.0).fract();
            let jy = (hh * 71.3 + 0.37).fract();
            let px = ((cx as f32 + jx) * sp).floor() as i32;
            let py = ((cy as f32 + jy) * sp).floor() as i32;
            if px < 0 || py < 0 || px >= wi || py >= hi {
                continue;
            }
            let s = 0.45 + 0.55 * (hh - 0.86) / 0.14;
            let col = star_tint((hh * 313.0).fract());
            let idx = ((py as u32 * w + px as u32) * 4) as usize;
            out[idx] = (clamp01(out[idx] as f32 / 255.0 + s * col[0]) * 255.0) as u8;
            out[idx + 1] = (clamp01(out[idx + 1] as f32 / 255.0 + s * col[1]) * 255.0) as u8;
            out[idx + 2] = (clamp01(out[idx + 2] as f32 / 255.0 + s * col[2]) * 255.0) as u8;
        }
    }
}

/// Dot in a moon's orbit path as a faint dashed ellipse around the parent.
/// `width` (px) thickens each dash by stamping a filled square around every
/// sampled point; `width == 1.0` collapses to the original single-pixel dot.
fn paint_orbit(out: &mut [u8], w: u32, h: u32, cam: &Camera, m: &Moon, width: f32) {
    // Square stamp half-extent: r == 0 at width 1 (pixel-identical to before).
    let r = (((width - 1.0) * 0.5).round()) as i32;
    let steps = 200;
    for k in 0..steps {
        // Dashed: skip every few samples.
        if (k / 3) % 2 == 0 {
            continue;
        }
        let a = TAU * k as f32 / steps as f32;
        let (s, c) = a.sin_cos();
        let wx = c * m.orbit;
        let wy = s * m.orbit * ORBIT_FLATTEN * m.tilt;
        let (sx, sy) = to_screen(wx, wy, cam, w, h);
        let (px, py) = (sx as i32, sy as i32);
        // Stamp a filled square centred on the sample, bounds-checked per pixel.
        for dy in -r..=r {
            for dx in -r..=r {
                let (sxp, syp) = (px + dx, py + dy);
                if sxp < 0 || syp < 0 || sxp >= w as i32 || syp >= h as i32 {
                    continue;
                }
                let idx = ((syp as u32 * w + sxp as u32) * 4) as usize;
                // Additive faint blue-grey.
                out[idx] = (out[idx] as u32 + 22).min(84) as u8;
                out[idx + 1] = (out[idx + 1] as u32 + 26).min(90) as u8;
                out[idx + 2] = (out[idx + 2] as u32 + 34).min(112) as u8;
            }
        }
    }
}

/// Alpha-blend a tile centred at screen `(sx, sy)` into the RGBA `out`,
/// nearest-neighbour scaled by `scale` (1.0 = 1:1). `scale > 1` blows each tile
/// pixel up into a crisp `scale`×`scale` block, so a body rendered into a small
/// tile turns blocky rather than blurry when magnified.
fn blit(out: &mut [u8], w: u32, h: u32, tile: &Tile, sx: f32, sy: f32, scale: f32) {
    let dsize = (tile.size as f32 * scale).round().max(1.0) as i32;
    let x0 = (sx - dsize as f32 * 0.5).floor() as i32;
    let y0 = (sy - dsize as f32 * 0.5).floor() as i32;
    let inv = 1.0 / scale;
    // Iterate only the on-screen slice of the destination rectangle — clamping
    // the loop bounds keeps blit cost proportional to visible area.
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

// Browser (wasm) C-ABI glue — excluded from native builds. See wasm.rs.
#[cfg(target_arch = "wasm32")]
mod wasm;
