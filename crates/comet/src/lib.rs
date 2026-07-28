//! comet — a procedural, seed-driven **comet** sweeping a real eccentric orbit.
//!
//! Pure math over the dependency-free `*-core` rlibs. Where `solar` lays out a
//! whole system of near-circular worlds, `comet` zooms in on the drama of a
//! single icy visitor: one star at a focus and a comet on a genuinely *eccentric*
//! ellipse, streaming a glowing tail that always points directly away from the
//! star. Same seed => the same comet on the same orbit, forever.
//!
//! This crate leans on the workspace's shared, dependency-free `*-core` rlibs:
//! `noise-core`/`dither-core` for the compact noise/color/dither primitives,
//! `scene-core` for compositor helpers, and `sun-core` for the tile renderer of
//! the small emissive star. The new work here is the physics-flavoured layer on
//! top:
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
// Noise + math primitives — now imported from the shared, dependency-free
// crates (`noise-core`, `dither-core`). These were previously copy-pasted here
// byte-for-byte; the shared versions are numerically identical, so output is
// unchanged.
// ===========================================================================

use dither_core::bayer;
use noise_core::{clamp01, fbm, hash3, Rgb};

// The shared deep-space backdrop, common to every scene crate.
use background_core::{paint_backdrop, paint_stars, Backdrop, StarLayer, StarTints, Starfield};

// Shared scene-compositor primitives (were previously copy-pasted here
// byte-for-byte). Camera is part of this crate's public API, so re-export it.
use scene_core::{blit, to_screen, visible_tile_rect, Rng, Tile, ORBIT_FLATTEN};
use std::cell::RefCell;
pub use scene_core::Camera;

// The compact star renderer is shared with `solar` via `sun-core`; comet keeps
// only its own `STARS` table of `StarKind` archetypes.
use sun_core::StarKind;

/// Bounded, decorrelated per-body noise offsets — keep them small so f32
/// precision holds and the noise doesn't collapse into bands. Thin wrapper over
/// the shared `noise_core::seed_offsets` with this crate's 220.0 span, so every
/// call site and its exact numeric result are unchanged.
fn seed_offsets(seed: u32) -> [f32; 3] {
    noise_core::seed_offsets(seed, 220.0)
}

// ===========================================================================
// The star at the focus
// ===========================================================================

// The star archetype (`StarKind`) and its renderer now live in `sun-core`; this
// crate keeps only its own table of archetypes below.
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

/// Render the star to a tile of diameter ~`2*rad_px` (+corona margin).
///
/// Thin wrapper over the shared `sun_core::render_star_tile_into`: comet always
/// renders full detail (no LOD) and uses its own `CORONA_REACH`.
fn render_star_tile(tile: &mut Tile, sk: &StarKind, seed: u32, t: f32, rad_px: f32, clip: [u32; 4]) {
    sun_core::render_star_tile_into(tile, sk, seed, t, rad_px, CORONA_REACH, false, clip);
}

// ===========================================================================
// The comet + its orbit
// ===========================================================================

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
    /// Reused star-tile buffer. The star is baked fresh every frame, so without
    /// this each one allocates and zero-fills a tile that at the 120 px render
    /// cap is ~800 KB. Interior-mutable because `render` takes `&self`.
    star_tile: RefCell<Tile>,
    pub star_kind: usize,
    pub star_radius: f32, // world units
    pub comets: Vec<Comet>,
    /// Stroke width (px) of the dashed orbit ellipse; 1.0 = the original look.
    pub orbit_width: f32,
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

        CometScene { seed, star_kind, star_radius, comets, orbit_width: 1.0, star_tile: RefCell::default() }
    }

    /// Set the orbit-ellipse stroke width in pixels, clamped to 1..=6.
    pub fn set_orbit_width(&mut self, px: f32) {
        self.orbit_width = px.clamp(1.0, 6.0);
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
            paint_orbit(out, w, h, cam, c, self.orbit_width);
        }

        // Star tile at the world origin (the focus).
        let sk = &STARS[self.star_kind];
        let (starx, stary) = to_screen(0.0, 0.0, cam, w, h);
        let rad_px = self.star_radius * cam.zoom;
        if rad_px >= 0.5 {
            let rad_render = rad_px.clamp(2.0, 120.0);
            let scale = rad_px / rad_render;
            // Shade only what the compositor will read back: zoomed onto the
            // star, most of its tile hangs off the viewport.
            let tsize = sun_core::star_tile_size(rad_render, CORONA_REACH);
            let clip = visible_tile_rect(tsize, w, h, starx, stary, scale);
            if clip[2] != clip[0] {
                let mut tile = self.star_tile.borrow_mut();
                render_star_tile(&mut tile, sk, self.seed, t, rad_render, clip);
                blit(out, w, h, &tile, starx, stary, scale);
            }
        }

        for c in &self.comets {
            draw_comet(out, w, h, cam, c, starx, stary, t);
        }
    }
}

// ===========================================================================
// Backdrop + orbit path
// ===========================================================================

/// This scene's sky: mostly pale/blue-white, a few warm.
const STAR_TINTS: StarTints = &[
    (0.50, [0.92, 0.95, 1.00]),
    (0.72, [0.72, 0.83, 1.00]),
    (0.88, [1.00, 0.96, 0.78]),
    (1.01, [1.00, 0.82, 0.60]),
];

/// Two layers on a slow pan-parallax. Deliberately quieter than `solar`'s three
/// plus a nebula: the comet is the subject, so the background stays out of the way.
const STAR_LAYERS: &[StarLayer] = &[
    StarLayer { parallax: 0.12, spacing: 7.0, threshold: 0.86, brightness: 0.55, faint: 0.5, salt: 0 },
    StarLayer { parallax: 0.30, spacing: 10.0, threshold: 0.88, brightness: 0.90, faint: 0.5, salt: 1 },
];

/// Dithered navy, no nebula — see above.
const BACKDROP: Backdrop = Backdrop {
    base: [0.028, 0.026, 0.060],
    dither: 0.012,
    nebula: None,
};

/// Paint the space backdrop: the navy ground plus a faint hashed starfield
/// anchored in screen space, scrolling at a slow pan-parallax tied to the camera.
fn paint_background(out: &mut [u8], w: u32, h: u32, cam: &Camera, seed: u32) {
    let (bgx, bgy) = (cam.x * cam.zoom, cam.y * cam.zoom);
    paint_backdrop(out, w, h, &BACKDROP, seed, bgx, bgy, 1.0, 0.0, None);
    let sky = Starfield::new(STAR_LAYERS, STAR_TINTS);
    // Salt the grid with the seed so each scene gets its own sky.
    let si = seed as i32;
    paint_stars(out, w, h, &sky, bgx, bgy, |cx, cy, salt| {
        hash3(cx.wrapping_add(si), cy, 17 + salt)
    });
}

/// Dot in a comet's orbit as a faint dashed ellipse around the star. Samples the
/// true Keplerian ellipse (uniform in eccentric anomaly), so the drawn path
/// exactly matches where the comet travels.
fn paint_orbit(out: &mut [u8], w: u32, h: u32, cam: &Camera, c: &Comet, width: f32) {
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
        // Stamp a filled square of half-extent `r` centred on the point; each
        // pixel is bounds-checked here, so the centre needs no separate check.
        // `width == 1.0` → `r == 0` → a single pixel, identical to before.
        let r = (((width - 1.0) * 0.5).round()) as i32;
        for dy in -r..=r {
            for dx in -r..=r {
                let (sx2, sy2) = (px + dx, py + dy);
                if sx2 < 0 || sy2 < 0 || sx2 >= w as i32 || sy2 >= h as i32 {
                    continue;
                }
                add_px(out, w, sx2 as u32, sy2 as u32, [0.10, 0.12, 0.18]);
            }
        }
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
