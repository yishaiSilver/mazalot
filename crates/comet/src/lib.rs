//! comet — a procedural, seed-driven **comet** sweeping a real eccentric orbit.
//!
//! Pure math, zero dependencies. Where `solar` lays out a whole system of near-
//! circular worlds, `comet` zooms in on the drama of a single icy visitor: one
//! star at a focus and a comet on a genuinely *eccentric* ellipse, streaming a
//! glowing tail that always points directly away from the star. Same seed => the
//! same comet on the same orbit, forever.
//!
//! This crate is self-contained by the workspace rule (each "type" crate shares
//! no code with the others — only third-party deps and the manifest). It carries
//! its own compact noise/color/dither primitives and its own tile renderer for a
//! small emissive star; the new work here is the physics-flavoured layer on top:
//!
//!   * a **Keplerian orbit** — the comet's position comes from solving Kepler's
//!     equation (`M = E − e·sin E`) each frame, so it genuinely SPEEDS UP through
//!     perihelion and crawls at aphelion (Kepler's 2nd law falls out for free),
//!     not a uniform sweep faked with a sine;
//!   * an **anti-sunward tail** — exactly the screen-space star→body direction
//!     `solar` uses to light its planets, here reused to aim the tail: a straight
//!     bluish ion plume dead-radially-outward plus a curved yellow dust plume
//!     that lags along the orbit, both lengthening and brightening as `1/dist`
//!     near perihelion.
//!
//! Pipeline per frame (see [`CometScene::render`]):
//!   1. paint the space backdrop (navy + a faint hashed starfield),
//!   2. dot in each comet's dashed elliptical orbit path,
//!   3. blit the central star (emissive disc + soft corona),
//!   4. stream each comet's tails as additive, fbm-modulated plumes, then cap the
//!      head with a fuzzy coma and a bright nucleus.
//!
//! The heavy cost is the tail splatting, and it is bounded (a fixed number of
//! soft discs, each a small clamped radius), so the whole scene stays cheap
//! enough to render live every frame — the "bake-or-stay-small" guidance in the
//! workspace README.

use std::f32::consts::{PI, TAU};

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
/// the star its convection cells.
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
    x.clamp(0.0, 1.0)
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

/// Tiny deterministic RNG for scene generation (SplitMix-ish over hash3).
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
}

// ===========================================================================
// Ordered dither (crisp pixel-art read)
// ===========================================================================

/// 8x8 Bayer matrix for ordered dithering, in −0.5..0.5 once normalized.
const BAYER: [u8; 64] = [
    0, 32, 8, 40, 2, 34, 10, 42, 48, 16, 56, 24, 50, 18, 58, 26, 12, 44, 4, 36, 14, 46,
    6, 38, 60, 28, 52, 20, 62, 30, 54, 22, 3, 35, 11, 43, 1, 33, 9, 41, 51, 19, 59, 27,
    49, 17, 57, 25, 15, 47, 7, 39, 13, 45, 5, 37, 63, 31, 55, 23, 61, 29, 53, 21,
];
fn bayer(x: u32, y: u32) -> f32 {
    (BAYER[((y % 8) * 8 + (x % 8)) as usize] as f32 + 0.5) / 64.0 - 0.5
}

/// Ordered-dither quantize a colour to kill banding while staying crisp under
/// motion. `bx` is the Bayer offset for this pixel.
fn quant(o: Rgb, bx: f32) -> Rgb {
    let levels = 24.0;
    let d = bx * 0.7 / levels;
    [
        clamp01(((o[0] + d) * levels).round() / levels),
        clamp01(((o[1] + d) * levels).round() / levels),
        clamp01(((o[2] + d) * levels).round() / levels),
    ]
}

// ===========================================================================
// The star at the focus
// ===========================================================================

/// A compact star archetype for the orbital focus: a cool→mid→hot photosphere
/// ramp plus a corona tint. Two tints is plenty for a comet showcase — the star
/// is a supporting player here, the comet is the star (so to speak).
#[derive(Clone, Copy)]
struct StarKind {
    name: &'static str,
    cool: Rgb,
    mid: Rgb,
    hot: Rgb,
    corona: Rgb,
    gran: f32, // granulation cell frequency
}

const STARS: &[StarKind] = &[
    StarKind { name: "yellow star", cool: [0.55, 0.20, 0.02], mid: [0.99, 0.74, 0.20], hot: [1.0, 0.97, 0.82], corona: [1.0, 0.82, 0.42], gran: 5.5 },
    StarKind { name: "white star",  cool: [0.48, 0.56, 0.85], mid: [0.87, 0.91, 1.0],  hot: [1.0, 1.0, 1.0],   corona: [0.82, 0.90, 1.0], gran: 6.5 },
];

/// Number of star archetypes.
pub fn star_kind_count() -> usize {
    STARS.len()
}
/// Name of a star archetype (wraps out of range).
pub fn star_kind_name(i: usize) -> &'static str {
    STARS[i % STARS.len()].name
}

/// Radius of the corona halo past the disc, in disc radii.
const CORONA_REACH: f32 = 0.85;

/// Emissive photosphere colour at a rotated surface point + limb factor `mu`.
fn star_surface(sk: &StarKind, sx: f32, sy: f32, sz: f32, ofs: [f32; 3], t: f32, mu: f32) -> Rgb {
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

/// A rendered star ready to blit: RGBA pixels + tile diameter. Alpha is 0 off
/// the halo, 255 on the opaque disc, and partial in the soft corona.
struct Tile {
    px: Vec<u8>,
    size: u32,
}

/// Render the star to a tile of diameter ~`2*rad_px` (+corona margin).
fn render_star_tile(sk: &StarKind, seed: u32, t: f32, rad_px: f32) -> Tile {
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
                col = star_surface(sk, nx, ny, nz, ofs, t, nz);
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

// ===========================================================================
// The comet + its orbit
// ===========================================================================

/// How much orbits are squashed vertically to fake a tilted, near-top-down view
/// (shared with `solar`'s `ORBIT_FLATTEN`).
const ORBIT_FLATTEN: f32 = 0.42;

/// Warm/cool dust-tail tints picked per comet by seed. The ion tail is always a
/// cold electric blue; only the dust plume varies.
const DUST_TINTS: &[Rgb] = &[
    [1.00, 0.86, 0.48], // amber
    [1.00, 0.74, 0.42], // orange
    [0.96, 0.92, 0.64], // pale gold
    [1.00, 0.80, 0.60], // peach
];

/// One comet on a genuine eccentric ellipse with the star at a focus.
///
/// The orbit lives in a plane that is then squashed vertically ([`ORBIT_FLATTEN`])
/// for the tilted look. All lengths are **world units**; angles are radians.
/// Position at a time `t` is *not* uniform in angle — it comes from solving
/// Kepler's equation, so the comet sweeps fast at perihelion and slow at
/// aphelion (Kepler's 2nd law).
#[derive(Clone, Copy)]
pub struct Comet {
    pub a: f32,      // semi-major axis, world units
    pub e: f32,      // eccentricity (0 = circle, →1 = very elongated)
    pub arg: f32,    // argument of periapsis — orientation of the ellipse
    pub period: f32, // time for one full orbit
    pub phase: f32,  // mean anomaly at t = 0
    pub tilt: f32,   // extra orbit foreshortening (1 = full ORBIT_FLATTEN squash)
    pub nucleus: f32, // nucleus radius, world units
    pub tint: usize, // index into DUST_TINTS
    pub seed: u32,   // this comet's noise seed
}

/// Solve Kepler's equation `M = E − e·sin E` for the eccentric anomaly `E`.
///
/// This is the heart of the non-uniform motion: `M` (the mean anomaly) advances
/// perfectly uniformly with time, but `E` — and hence the real position — does
/// not, which is precisely why the comet accelerates through perihelion. A few
/// Newton steps converge to well under a pixel for any sane eccentricity.
fn solve_kepler(m: f32, e: f32) -> f32 {
    // Wrap M into [−π, π] for fast, stable Newton convergence.
    let m = m - TAU * (m / TAU + 0.5).floor();
    let mut ea = if e < 0.8 { m } else { PI.copysign(m) };
    for _ in 0..6 {
        let f = ea - e * ea.sin() - m;
        let fp = 1.0 - e * ea.cos();
        ea -= f / fp;
    }
    ea
}

impl Comet {
    /// Semi-minor axis.
    fn b(&self) -> f32 {
        self.a * (1.0 - self.e * self.e).sqrt()
    }
    /// Closest approach distance (perihelion), world units.
    pub fn perihelion(&self) -> f32 {
        self.a * (1.0 - self.e)
    }
    /// Farthest distance (aphelion), world units.
    pub fn aphelion(&self) -> f32 {
        self.a * (1.0 + self.e)
    }

    /// World position at time `t` **and** the true star-distance there.
    ///
    /// Returns `(wx, wy, dist)`: `wx, wy` are the squashed screen-plane world
    /// coordinates (star at the origin is a focus); `dist` is the *unsquashed*
    /// orbital radius `a(1 − e·cos E)`, used to drive tail length/brightness so
    /// activity tracks true proximity, not the foreshortened on-screen gap.
    fn state(&self, t: f32) -> (f32, f32, f32) {
        let m = self.phase + TAU * t / self.period;
        let ea = solve_kepler(m, self.e);
        let (se, ce) = ea.sin_cos();
        // Ellipse in its own plane, focus at origin, perihelion along +x.
        let ox = self.a * (ce - self.e);
        let oy = self.b() * se;
        let dist = self.a * (1.0 - self.e * ce);
        // Rotate by the argument of periapsis, then squash vertically for tilt.
        let (sw, cw) = self.arg.sin_cos();
        let rx = ox * cw - oy * sw;
        let ry = ox * sw + oy * cw;
        (rx, ry * ORBIT_FLATTEN * self.tilt, dist)
    }

    /// World position at time `t` (public convenience; drops the distance).
    pub fn pos(&self, t: f32) -> (f32, f32) {
        let (x, y, _) = self.state(t);
        (x, y)
    }
}

// ===========================================================================
// Scene
// ===========================================================================

/// A whole generated comet scene: one star at the focus and 1–3 comets on
/// eccentric orbits around it. Deterministic in `seed`.
pub struct CometScene {
    pub seed: u32,
    pub star_kind: usize,
    pub star_radius: f32, // world units
    pub comets: Vec<Comet>,
}

impl CometScene {
    /// Build the scene for `seed` with the seed-derived comet count (1..=3).
    pub fn generate(seed: u32) -> CometScene {
        CometScene::generate_n(seed, 0)
    }

    /// Build the scene for `seed`, forcing the comet count when `count > 0`
    /// (0 keeps the seed-derived 1..=3, clamped to 1..=3). The auto count is
    /// still drawn from the RNG either way, so the shared comets are identical
    /// whether or not the count is forced.
    pub fn generate_n(seed: u32, count_override: u32) -> CometScene {
        let mut rng = Rng::new(seed ^ 0x0000_c0e7);
        let star_kind = (rng.f() * STARS.len() as f32) as usize % STARS.len();
        let star_radius = rng.range(22.0, 30.0);

        let auto = 1 + (rng.f() * 3.0) as usize; // 1..=3
        let count = if count_override > 0 {
            (count_override as usize).clamp(1, 3)
        } else {
            auto.clamp(1, 3)
        };

        let mut comets = Vec::with_capacity(count);
        for i in 0..count {
            // Eccentric by design: e well away from 0 so the speed-up reads.
            let e = rng.range(0.58, 0.86);
            // Semi-major axis grows a little per comet so multiple orbits nest.
            let mut a = rng.range(150.0, 230.0) + i as f32 * 46.0;
            // Guarantee the perihelion clears the star + corona with margin.
            let peri_min = star_radius * (1.6 + CORONA_REACH) + 14.0;
            if a * (1.0 - e) < peri_min {
                a = peri_min / (1.0 - e);
            }
            let arg = rng.range(0.0, TAU);
            // Kepler's 3rd law flavour: bigger orbits take longer.
            let period = 8.0 * (a / 150.0).powf(1.5) * rng.range(0.9, 1.1);
            let phase = rng.range(0.0, TAU);
            let tilt = rng.range(0.82, 1.0);
            let nucleus = rng.range(2.4, 4.2);
            let tint = (rng.f() * DUST_TINTS.len() as f32) as usize % DUST_TINTS.len();
            let cseed = seed.wrapping_mul(2_654_435_761).wrapping_add(i as u32 * 40_503 + 1);
            comets.push(Comet { a, e, arg, period, phase, tilt, nucleus, tint, seed: cseed });
        }

        CometScene { seed, star_kind, star_radius, comets }
    }

    /// Outermost extent (world units) — the largest aphelion — for zoom-to-fit.
    pub fn extent(&self) -> f32 {
        self.comets
            .iter()
            .map(|c| c.aphelion())
            .fold(self.star_radius, f32::max)
            + 30.0
    }

    /// Render the whole scene into `out` (RGBA, `w*h*4` bytes) at time `t`.
    ///
    /// Draw order: backdrop → orbit paths → star → each comet's tails (additive)
    /// → coma + nucleus. Tails are drawn over the star's corona intentionally:
    /// they glow, and they point away from the star anyway, so overlap is minimal.
    pub fn render(&self, w: u32, h: u32, cam: &Camera, t: f32, out: &mut [u8]) {
        assert!(out.len() >= (w * h * 4) as usize);
        paint_background(out, w, h, cam, self.seed);
        for c in &self.comets {
            paint_orbit(out, w, h, cam, c);
        }

        // Star tile at the world origin (the focus).
        let sk = &STARS[self.star_kind];
        let (starx, stary) = to_screen(0.0, 0.0, cam, w, h);
        let rad_px = self.star_radius * cam.zoom;
        if rad_px >= 0.5 {
            let rad_render = rad_px.clamp(2.0, 120.0);
            let tile = render_star_tile(sk, self.seed, t, rad_render);
            blit(out, w, h, &tile, starx, stary, rad_px / rad_render);
        }

        for c in &self.comets {
            draw_comet(out, w, h, cam, c, starx, stary, t);
        }
    }
}

// ===========================================================================
// Camera + screen mapping
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
// Backdrop + orbit path
// ===========================================================================

/// Star colour by a hash in [0,1): mostly pale/blue-white, a few warm.
fn star_tint(hh: f32) -> Rgb {
    if hh < 0.5 {
        [0.92, 0.95, 1.00]
    } else if hh < 0.72 {
        [0.72, 0.83, 1.00]
    } else if hh < 0.88 {
        [1.00, 0.96, 0.78]
    } else {
        [1.00, 0.82, 0.60]
    }
}

/// Paint the space backdrop: a base navy plus a faint, fixed hashed starfield.
///
/// The field is anchored in SCREEN space with a slow pan-parallax tied to the
/// camera, plotted by iterating visible grid cells (O(cells)) — one 1px star
/// per hit. Deliberately simpler than `solar`'s nebula layers: the comet is the
/// subject, so the background stays quiet.
fn paint_background(out: &mut [u8], w: u32, h: u32, cam: &Camera, seed: u32) {
    // Pass 1: flat navy base.
    for iy in 0..h {
        for ix in 0..w {
            let d = bayer(ix, iy) * 0.012;
            let idx = ((iy * w + ix) * 4) as usize;
            out[idx] = (clamp01(0.028 + d) * 255.0) as u8;
            out[idx + 1] = (clamp01(0.026 + d) * 255.0) as u8;
            out[idx + 2] = (clamp01(0.060 + d) * 255.0) as u8;
            out[idx + 3] = 255;
        }
    }

    // Pass 2: two star layers scrolled at a slow pan-parallax rate.
    let si = seed as i32;
    let (bgx, bgy) = (cam.x * cam.zoom, cam.y * cam.zoom);
    let (wi, hi) = (w as i32, h as i32);
    // (parallax p, screen grid px, threshold, brightness, salt)
    let layers: [(f32, f32, f32, f32, i32); 2] = [
        (0.12, 7.0, 0.86, 0.55, 0),  // far — dim
        (0.30, 10.0, 0.88, 0.90, 1), // near — brighter
    ];
    for (p, sp, thr, bri, salt) in layers {
        let inv = 1.0 / sp;
        let (ox, oy) = (bgx * p, bgy * p);
        let (c0x, c1x) = ((ox * inv).floor() as i32 - 1, ((ox + w as f32) * inv).floor() as i32 + 1);
        let (c0y, c1y) = ((oy * inv).floor() as i32 - 1, ((oy + h as f32) * inv).floor() as i32 + 1);
        for cy in c0y..=c1y {
            for cx in c0x..=c1x {
                let hh = hash3(cx.wrapping_add(si), cy, 17 + salt);
                if hh <= thr {
                    continue;
                }
                let jx = (hh * 137.0).fract();
                let jy = (hh * 71.3 + 0.37).fract();
                let px = ((cx as f32 + jx) * sp - ox).floor() as i32;
                let py = ((cy as f32 + jy) * sp - oy).floor() as i32;
                if px < 0 || py < 0 || px >= wi || py >= hi {
                    continue;
                }
                let tt = (hh - thr) / (1.0 - thr);
                let s = bri * (0.5 + 0.5 * tt);
                let col = star_tint((hh * 313.0).fract());
                add_px(out, w, px as u32, py as u32, [col[0] * s, col[1] * s, col[2] * s]);
            }
        }
    }
}

/// Dot in a comet's orbit as a faint dashed ellipse around the star. Samples the
/// true Keplerian ellipse (uniform in eccentric anomaly), so the drawn path
/// exactly matches where the comet travels.
fn paint_orbit(out: &mut [u8], w: u32, h: u32, cam: &Camera, c: &Comet) {
    let steps = 260;
    let (sw, cw) = c.arg.sin_cos();
    let b = c.b();
    for k in 0..steps {
        // Dashed: skip every few samples.
        if (k / 3) % 2 == 0 {
            continue;
        }
        let ea = TAU * k as f32 / steps as f32;
        let (se, ce) = ea.sin_cos();
        let ox = c.a * (ce - c.e);
        let oy = b * se;
        let rx = ox * cw - oy * sw;
        let ry = (ox * sw + oy * cw) * ORBIT_FLATTEN * c.tilt;
        let (sx, sy) = to_screen(rx, ry, cam, w, h);
        let (px, py) = (sx as i32, sy as i32);
        if px < 0 || py < 0 || px >= w as i32 || py >= h as i32 {
            continue;
        }
        add_px(out, w, px as u32, py as u32, [0.10, 0.12, 0.18]);
    }
}

// ===========================================================================
// Comet: tails, coma, nucleus (all additive)
// ===========================================================================

/// Additive-blend an RGB glow into one pixel (rgb in 0..1), saturating at 255.
#[inline]
fn add_px(out: &mut [u8], w: u32, x: u32, y: u32, col: Rgb) {
    let idx = ((y * w + x) * 4) as usize;
    out[idx] = (out[idx] as u32 + (clamp01(col[0]) * 255.0) as u32).min(255) as u8;
    out[idx + 1] = (out[idx + 1] as u32 + (clamp01(col[1]) * 255.0) as u32).min(255) as u8;
    out[idx + 2] = (out[idx + 2] as u32 + (clamp01(col[2]) * 255.0) as u32).min(255) as u8;
}

/// Splat one soft, additive disc of radius `rad` px centred at `(cx, cy)`, with
/// a smooth quadratic falloff to the rim and Bayer-dithered edges. The building
/// block for both tails and the coma.
fn splat(out: &mut [u8], w: u32, h: u32, cx: f32, cy: f32, rad: f32, col: Rgb) {
    if rad < 0.6 {
        let (px, py) = (cx as i32, cy as i32);
        if px >= 0 && py >= 0 && px < w as i32 && py < h as i32 {
            add_px(out, w, px as u32, py as u32, col);
        }
        return;
    }
    let x0 = ((cx - rad).floor() as i32).max(0);
    let x1 = ((cx + rad).ceil() as i32).min(w as i32);
    let y0 = ((cy - rad).floor() as i32).max(0);
    let y1 = ((cy + rad).ceil() as i32).min(h as i32);
    let inv_r2 = 1.0 / (rad * rad);
    for py in y0..y1 {
        for px in x0..x1 {
            let dx = px as f32 + 0.5 - cx;
            let dy = py as f32 + 0.5 - cy;
            let q = (dx * dx + dy * dy) * inv_r2;
            if q >= 1.0 {
                continue;
            }
            // Smooth core-bright falloff; dither so faint tiers don't band.
            let fall = (1.0 - q).powf(1.6) + bayer(px as u32, py as u32) * 0.04;
            if fall <= 0.0 {
                continue;
            }
            add_px(out, w, px as u32, py as u32, [col[0] * fall, col[1] * fall, col[2] * fall]);
        }
    }
}

/// The cold electric-blue ion tail colour (constant across comets).
const ION_TINT: Rgb = [0.42, 0.60, 1.0];
/// The whitish-cyan coma colour.
const COMA_TINT: Rgb = [0.70, 0.86, 1.0];

/// Draw one comet: its two tails (streamed as additive plumes pointing directly
/// away from the star), then the fuzzy coma and bright nucleus over the head.
#[allow(clippy::too_many_arguments)]
fn draw_comet(out: &mut [u8], w: u32, h: u32, cam: &Camera, c: &Comet, starx: f32, stary: f32, t: f32) {
    let (wx, wy, dist) = c.state(t);
    let (hx, hy) = to_screen(wx, wy, cam, w, h);

    // --- anti-sunward direction (exactly solar's star→body screen vector) ---
    let dx = hx - starx;
    let dy = hy - stary;
    let dmag = (dx * dx + dy * dy).sqrt().max(1e-3);
    let out_dir = (dx / dmag, dy / dmag); // unit, points radially AWAY from star

    // --- screen-space velocity, for the dust tail's trailing curve ---
    let dt = c.period * 0.004;
    let (wx2, wy2, _) = c.state(t + dt);
    let (hx2, hy2) = to_screen(wx2, wy2, cam, w, h);
    let (vx, vy) = (hx2 - hx, hy2 - hy);
    let vmag = (vx * vx + vy * vy).sqrt().max(1e-3);
    let trail = (-vx / vmag, -vy / vmag); // opposite of motion
    // Component of `trail` perpendicular to `out_dir` — the way the dust bends.
    let tdot = trail.0 * out_dir.0 + trail.1 * out_dir.1;
    let mut perp = (trail.0 - out_dir.0 * tdot, trail.1 - out_dir.1 * tdot);
    let pmag = (perp.0 * perp.0 + perp.1 * perp.1).sqrt();
    if pmag > 1e-3 {
        perp = (perp.0 / pmag, perp.1 / pmag);
    }

    // --- activity: everything scales with proximity to the star (~1/dist) ---
    // 1 near perihelion, small at aphelion. This is the whole "brighter and
    // longer near the star" behaviour, in one number.
    let peri = c.perihelion();
    let activity = clamp01((peri / dist).powf(1.3));
    let z = cam.zoom;
    let ofs = seed_offsets(c.seed);

    // ---- ion tail: straight, narrow, blue, longest ----
    let ion_len = (150.0 * z) * (0.25 + 0.9 * activity);
    let ion_w0 = (2.2 * z).max(1.0);
    stream_tail(
        out, w, h, (hx, hy), out_dir, (0.0, 0.0), 0.0, ion_len, ion_w0, 1.35,
        ION_TINT, 0.9 * activity + 0.08, ofs, t, 11.0,
    );

    // ---- dust tail: shorter, wider, warmer, curved along the orbit ----
    let dust_len = (105.0 * z) * (0.25 + 0.9 * activity);
    let dust_w0 = (3.4 * z).max(1.2);
    let curve = 0.42; // how far the plume bends toward the trailing direction
    stream_tail(
        out, w, h, (hx, hy), out_dir, perp, curve, dust_len, dust_w0, 2.1,
        DUST_TINTS[c.tint], 0.8 * activity + 0.06, ofs, t, 5.0,
    );

    // ---- coma: a fuzzy glow around the head, brightening near the star ----
    let coma_r = (c.nucleus * z * 3.4) * (0.7 + 0.8 * activity);
    splat(out, w, h, hx, hy, coma_r.max(2.0), [
        COMA_TINT[0] * (0.4 + 0.5 * activity),
        COMA_TINT[1] * (0.4 + 0.5 * activity),
        COMA_TINT[2] * (0.4 + 0.5 * activity),
    ]);

    // ---- nucleus: a small, near-white bright core ----
    let nuc_r = (c.nucleus * z).max(1.0);
    splat(out, w, h, hx, hy, nuc_r, [1.0, 0.98, 0.92]);
}

/// Stream one tapering, fbm-modulated plume from the head outward.
///
/// The plume is a fixed budget of soft additive discs marched along an axis:
/// `dir` is the (unit) outward axis and `perp` a lateral bend direction whose
/// magnitude grows with `curve · s²` (0 = a dead-straight tail). Width fans from
/// `w0` outward; brightness fades along the length and is roughened by fbm so
/// the plume shimmers. Cost is bounded — a clamped step count of clamped-radius
/// discs — so it is cheap every frame.
#[allow(clippy::too_many_arguments)]
fn stream_tail(
    out: &mut [u8],
    w: u32,
    h: u32,
    head: (f32, f32),
    dir: (f32, f32),
    perp: (f32, f32),
    curve: f32,
    len: f32,
    w0: f32,
    width_fan: f32,
    tint: Rgb,
    intensity: f32,
    ofs: [f32; 3],
    t: f32,
    noise_freq: f32,
) {
    if len < 2.0 || intensity <= 0.01 {
        return;
    }
    let steps = ((len * 0.7) as u32).clamp(10, 130);
    for k in 0..=steps {
        let s = k as f32 / steps as f32; // 0 at head → 1 at tip
        // Position: straight march plus the curved lateral offset (dust only).
        let along = s * len;
        let bend = curve * len * s * s;
        let cx = head.0 + dir.0 * along + perp.0 * bend;
        let cy = head.1 + dir.1 * along + perp.1 * bend;
        // Width fans out along the tail; brightness fades toward the tip.
        let width = w0 * (0.4 + width_fan * s);
        // fbm makes the plume grainy + alive; slide the field along the tail
        // and drift it in time so the dust appears to stream outward.
        let n = fbm(
            ofs[0] + s * noise_freq - t * 1.2,
            ofs[1] + dir.0 * s * 4.0,
            ofs[2] + dir.1 * s * 4.0,
            3,
        );
        let fade = (1.0 - s).powf(1.5);
        let bright = intensity * fade * (0.45 + 1.1 * n);
        if bright <= 0.01 {
            continue;
        }
        splat(out, w, h, cx, cy, width, [tint[0] * bright, tint[1] * bright, tint[2] * bright]);
    }
}

// ===========================================================================
// Tile blit (alpha over) — for the star
// ===========================================================================

/// Alpha-blend a tile centred at screen `(sx, sy)` into the RGBA `out`,
/// nearest-neighbour scaled by `scale` (1.0 = 1:1). `scale > 1` blows each tile
/// pixel up into a crisp block, applying pixelation without changing the
/// on-screen size.
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

/// World position of comet `i` at time `t` (for a camera that follows the head).
/// Returns `(0, 0)` — the star — for an out-of-range index.
pub fn comet_world_pos(scene: &CometScene, i: usize, t: f32) -> (f32, f32) {
    match scene.comets.get(i) {
        Some(c) => c.pos(t),
        None => (0.0, 0.0),
    }
}

// Browser (wasm) C-ABI glue — excluded from native builds. See wasm.rs.
#[cfg(target_arch = "wasm32")]
mod wasm;
