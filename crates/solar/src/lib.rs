//! solar — a procedural, seed-driven **solar system** you can drag around.
//!
//! Pure math over the dependency-free `*-core` rlibs. Where `planet` and `star`
//! each render one body filling a square, `solar` renders a *whole system* into
//! an arbitrary rectangular viewport: a central star with planets orbiting it,
//! drawn against a starfield you can pan across and zoom into. Same seed => the
//! same system, forever.
//!
//! This crate leans on the workspace's shared, dependency-free `*-core` rlibs:
//! `noise-core`/`dither-core` for the noise/color primitives, `scene-core` for the
//! compositor + camera helpers, `sun-core` for the compact star tile, and
//! `planet-core` for the planets. That last one is the whole point: the worlds in
//! orbit here are literally the worlds the `planet` demo renders — same archetype
//! table, same shader — asked for in its *sprite* framing (a transparent tile lit
//! from an arbitrary direction) instead of its hero framing. The new work here is
//! the layer on top — orbital layout, depth sorting so planets pass in front of
//! and behind the sun, and a draggable camera.
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

use std::cell::RefCell;
use std::f32::consts::TAU;

// ===========================================================================
// Shared primitives. Only the hash (for the star grid) and the smoothstep that
// drives the zoom fades are needed here — the backdrop's own noise and dither
// live in `background-core`.
// ===========================================================================

use noise_core::{hash3, smoothstep, Rgb};

// The shared deep-space backdrop: ground, nebula and parallax stars. Every scene
// crate paints through this; all that differs between them is the config below.
use background_core::{
    paint_backdrop, paint_stars, Backdrop, BackdropCache, Nebula, StarLayer, StarTints, Starfield,
};

// The scene-compositor kit: camera transform, seeded RNG, and the tile blitter
// every body is drawn through. `Camera` is re-exported because it's part of this
// crate's public API (the wasm glue + bins reference `solar::Camera`).
use scene_core::{blit, to_screen, visible_tile_rect, Rng, Tile, ORBIT_FLATTEN};
pub use scene_core::Camera;

// The compact star-tile renderer, shared with `comet`. Aliased to `SunKind`
// because in a system this star is *the* sun.
use sun_core::StarKind as SunKind;

// ===========================================================================
// The star at the centre
// ===========================================================================

const SUNS: &[SunKind] = &[
    SunKind { name: "yellow dwarf", cool: [0.55, 0.20, 0.02], mid: [0.99, 0.74, 0.20], hot: [1.0, 0.97, 0.82], corona: [1.0, 0.82, 0.42], gran: 5.5 },
    SunKind { name: "orange dwarf", cool: [0.35, 0.08, 0.01], mid: [0.96, 0.52, 0.14], hot: [1.0, 0.86, 0.54], corona: [1.0, 0.66, 0.30], gran: 5.0 },
    SunKind { name: "red giant",    cool: [0.26, 0.03, 0.02], mid: [0.88, 0.26, 0.09], hot: [1.0, 0.64, 0.30], corona: [1.0, 0.44, 0.20], gran: 4.0 },
    SunKind { name: "white star",   cool: [0.48, 0.56, 0.85], mid: [0.87, 0.91, 1.0],  hot: [1.0, 1.0, 1.0],   corona: [0.82, 0.90, 1.0], gran: 6.5 },
    SunKind { name: "blue giant",   cool: [0.10, 0.22, 0.60], mid: [0.47, 0.64, 1.0],  hot: [0.93, 0.98, 1.0], corona: [0.68, 0.84, 1.0], gran: 5.0 },
];

/// Radius of the corona halo past the disc, in disc radii.
const CORONA_REACH: f32 = 0.7;

/// Boil-clock quantum for the star-tile cache: `t_sun` is snapped to this step so
/// the tile is reused between re-bakes. Small enough that the convection looks
/// continuous (~a dozen steps/sec at the default rotation), large enough that the
/// costly tile is baked once every several frames instead of every frame.
const SUN_TQUANT: f32 = 0.08;

// ===========================================================================
// Planet roster
// ===========================================================================

/// One world this generator can place. The archetype itself — palette, shader,
/// weather, rings — belongs to `planet-core`, named here rather than indexed so
/// reordering that table can't silently re-point a row. All this crate adds is
/// the one fact a *system* needs and a single planet doesn't: where the world
/// belongs relative to its star. (Body size follows from `planet_core::is_giant`.)
struct Archetype {
    /// A `planet-core` type name (see `planet_core::type_name`).
    ty: &'static str,
    /// Preferred orbital band: 0 inner (hot/rocky), 1 mid, 2 outer (cold/gas).
    band: u8,
}

/// The worlds in play, grouped by band so systems read naturally (rock near the
/// star, gas and ice far out) without being rigid. Every `planet-core` archetype
/// appears exactly once — see the test at the bottom of this file.
///
/// The web demo's `PLANET_NAMES` is index-aligned with this table (it can't read
/// `&str`s across the C ABI), so reordering here means editing `web/index.html`.
const ROSTER: &[Archetype] = &[
    // inner — scorched, rocky, molten
    Archetype { ty: "desert", band: 0 },
    Archetype { ty: "barren", band: 0 },
    Archetype { ty: "moon", band: 0 },
    Archetype { ty: "iron", band: 0 },
    Archetype { ty: "obsidian", band: 0 },
    Archetype { ty: "lava", band: 0 },
    Archetype { ty: "molten_sea", band: 0 },
    // mid — the habitable belt, plus the odd exotic
    Archetype { ty: "terran", band: 1 },
    Archetype { ty: "ocean", band: 1 },
    Archetype { ty: "archipelago", band: 1 },
    Archetype { ty: "gaia", band: 1 },
    Archetype { ty: "swamp", band: 1 },
    Archetype { ty: "savanna", band: 1 },
    Archetype { ty: "toxic", band: 1 },
    Archetype { ty: "radioactive", band: 1 },
    Archetype { ty: "fungal", band: 1 },
    Archetype { ty: "chrome", band: 1 },
    // outer — frozen, banded, storm-wracked
    Archetype { ty: "ice", band: 2 },
    Archetype { ty: "tundra", band: 2 },
    Archetype { ty: "alpine", band: 2 },
    Archetype { ty: "crystal", band: 2 },
    Archetype { ty: "storm_shroud", band: 2 },
    Archetype { ty: "gas_giant", band: 2 },
    Archetype { ty: "ice_giant", band: 2 },
    Archetype { ty: "storm_giant", band: 2 },
    Archetype { ty: "ringed_giant", band: 2 },
];

/// `planet-core` type index for roster entry `i`. Resolved once per planet at
/// generation time and cached in [`Planet::ptype`], never per frame.
fn roster_type(i: usize) -> usize {
    planet_core::type_index(ROSTER[i % ROSTER.len()].ty).unwrap_or(0)
}

/// Number of planet archetypes.
pub fn planet_kind_count() -> usize {
    ROSTER.len()
}
/// Name of a planet archetype (wraps out of range).
pub fn planet_kind_name(i: usize) -> &'static str {
    ROSTER[i % ROSTER.len()].ty
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
    pub kind: usize,     // index into ROSTER (what the HUD names)
    pub ptype: usize,    // index into planet_core::TYPES (what renders it)
    pub orbit: f32,      // semi-major axis, world units
    pub radius: f32,     // body radius, world units
    pub speed: f32,      // mean motion, radians of mean anomaly per unit time
    pub phase: f32,      // mean anomaly at time 0
    pub tilt: f32,       // orbit foreshortening (0 = edge-on line, 1 = face-on circle)
    pub spin: f32,       // axial-spin turns per unit time (self rotation)
    pub e: f32,          // eccentricity (0 = circle, ..<1 = ellipse); star sits at a focus
    pub arg: f32,        // argument of periapsis — rotates the ellipse's long axis in-plane
    pub seed: u32,       // this body's noise seed
}

impl Planet {
    /// Effective eccentricity with the system's live `ecc` multiplier applied,
    /// clamped short of a parabola. `ecc == 0` forces perfect circles.
    #[inline]
    fn ecc(&self, ecc: f32) -> f32 {
        (self.e * ecc).clamp(0.0, 0.9)
    }

    /// In-plane point (focus at origin) at eccentric anomaly `ea`, rotated by the
    /// argument of periapsis. Returns `(x1, y1)` before the view squash. At `e ==
    /// 0` this is a circle of radius `orbit` about the sun (the old behaviour).
    #[inline]
    fn plane_point(&self, ea: f32, e: f32) -> (f32, f32) {
        let b = self.orbit * (1.0 - e * e).max(0.0).sqrt();
        let ox = self.orbit * (ea.cos() - e); // perihelion at ea=0; focus at origin
        let oy = b * ea.sin();
        let (sw, cw) = self.arg.sin_cos();
        (ox * cw - oy * sw, ox * sw + oy * cw)
    }

    /// World-space position + a depth key at time `t`. `spacing` scales the orbit,
    /// `ecc` scales eccentricity (both live UI multipliers). The mean anomaly
    /// `M = phase + speed·t` advances uniformly; the eccentric anomaly `E` is
    /// recovered from `M = E − e·sin E` by a few Newton steps (Kepler's 2nd law —
    /// the planet sweeps faster near perihelion). Depth > 0 means the near side of
    /// the orbit (drawn in front of the sun).
    fn at(&self, t: f32, spacing: f32, ecc: f32) -> (f32, f32, f32) {
        let e = self.ecc(ecc);
        let m = (self.phase + self.speed * t).rem_euclid(TAU);
        let mut ea = m;
        for _ in 0..6 {
            let f = ea - e * ea.sin() - m;
            let fp = 1.0 - e * ea.cos();
            ea -= f / fp;
        }
        let (x1, y1) = self.plane_point(ea, e);
        let x = x1 * spacing;
        let y = y1 * ORBIT_FLATTEN * self.tilt * spacing;
        (x, y, y1 / self.orbit) // depth ~ near/far side, normalised by the orbit size
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
    // Background star density (0 = none, 1 = default field, higher = more).
    pub star_density: f32,
    // Background pan-parallax rate multiplier (scales every layer's scroll rate
    // `p`; 0 = stars fixed on pan, 1 = default).
    pub star_parallax: f32,
    // Dashed orbit-path line thickness in pixels (1 = default 1px look).
    pub orbit_width: f32,
    // Eccentricity multiplier (scales every planet's generated `e`; 0 = force
    // perfect circles, 1 = as generated, higher = exaggerate the ellipses).
    pub ecc: f32,
    // Freeze each planet's weather into a baked map instead of evaluating it per
    // pixel per frame — ~2x on a cloudy world, ~2.6x on a shrouded one, at the
    // cost of the deck's billowing and its churning storm cells. The deck still
    // rotates over the surface. See `planet_core::F_BAKED_CLOUDS`.
    //
    // OFF by default, and deliberately: this is the one switch here that changes
    // the picture rather than the pixel budget, so the native generators keep the
    // animated deck and `out/` stays byte-identical. The web demo turns it on at
    // construction — it runs continuously and is shader-bound, which is the case
    // the trade was made for.
    pub frozen_clouds: bool,
    // Cached backdrop (background + orbit paths) + the key it was rendered for,
    // reused by `render_system_cached` while the camera/view is unchanged.
    bg_cache: Vec<u8>,
    bg_key: Option<BgKey>,
    // Cached backdrop layers (ground + nebula), owned by `background-core`.
    // Interior-mutable so it memoizes through the shared `&System` render path
    // too. See `paint_background`.
    neb: RefCell<BackdropCache>,
    // Cached star tile — the single most expensive body shader (27-cell worley +
    // fBm per pixel over a large tile when zoomed in). The boil evolves slowly,
    // so like the nebula its clock is quantized: on a still-ish sun the tile is
    // reused and only re-baked every few frames. See `draw_bodies`.
    sun_tile: RefCell<SunCache>,
    // Reused planet-tile scratch. Planets can't be cached the way the star is —
    // each is a different world and they all spin — but they can share a buffer.
    body_tile: RefCell<Tile>,
    // Reused draw-order scratch (avoids a per-frame Vec alloc in `draw_bodies`).
    order: RefCell<Vec<(f32, i32)>>,
}

/// Memoized star tile. The star's convection cells + corona are the costliest
/// per-pixel shader, but the boil is slow, so we key the tile on the render
/// radius and a QUANTIZED boil clock and reuse it between re-bakes.
///
/// The clip is part of the key: a clipped tile stops being valid the moment the
/// star moves somewhere that would show more of it.
#[derive(Default)]
struct SunCache {
    /// `[quantized rad_render, quantized t_sun, clip x0, y0, x1, y1]`.
    key: Option<[i32; 6]>,
    tile: Tile,
}

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
            // Pick a type whose band matches, else anything.
            let kind = pick_kind(&mut rng, want_band);
            let ptype = roster_type(kind);
            let radius = if planet_core::is_giant(ptype) {
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
            // A gentle spread of eccentricity so orbits read as ellipses with the
            // sun at a focus, not perfect circles — inner worlds a touch rounder,
            // the odd outer world more elongated. The `ecc` view knob scales this.
            let e = (rng.range(0.03, 0.24) + frac * rng.range(0.0, 0.18)).min(0.42);
            let arg = rng.range(0.0, TAU); // point each ellipse's long axis its own way
            let bseed = seed.wrapping_mul(2_654_435_761).wrapping_add(i as u32 * 40_503 + 1);
            planets.push(Planet {
                kind,
                ptype,
                orbit,
                radius,
                speed,
                phase,
                tilt,
                spin,
                e,
                arg,
                seed: bseed,
            });
            // Next orbit: leave room for this body + a growing gap.
            orbit += radius + rng.range(58.0, 96.0) + i as f32 * 8.0;
        }

        System {
            seed, sun_kind, sun_radius, planets,
            spacing: 1.0, planet_size: 1.0, sun_size: 1.0, planet_pixel: 1.0, sun_pixel: 1.0,
            planet_detail: 160.0, sun_detail: 110.0, star_density: 0.5, star_parallax: 1.0,
            orbit_width: 1.0, ecc: 1.0, frozen_clouds: false,
            bg_cache: Vec::new(), bg_key: None,
            neb: RefCell::new(BackdropCache::default()),
            sun_tile: RefCell::new(SunCache::default()),
            body_tile: RefCell::new(Tile::default()),
            order: RefCell::new(Vec::new()),
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
        star_density: f32,
        star_parallax: f32,
    ) {
        self.spacing = spacing.max(0.05);
        self.planet_size = planet_size.max(0.05);
        self.sun_size = sun_size.max(0.05);
        self.planet_pixel = planet_pixel.max(1.0);
        self.sun_pixel = sun_pixel.max(1.0);
        self.planet_detail = planet_detail.clamp(6.0, 256.0);
        self.sun_detail = sun_detail.clamp(6.0, 180.0);
        self.star_density = star_density.clamp(0.0, 4.0);
        self.star_parallax = star_parallax.clamp(0.0, 4.0);
    }

    /// Set the dashed orbit-path line thickness in pixels (1..=6; 1 = default).
    pub fn set_orbit_width(&mut self, px: f32) {
        self.orbit_width = px.clamp(1.0, 6.0);
    }

    /// Live eccentricity multiplier: 0 forces circular orbits, 1 keeps each
    /// planet's generated eccentricity, higher exaggerates the ellipses.
    pub fn set_eccentricity(&mut self, scale: f32) {
        self.ecc = scale.clamp(0.0, 2.5);
    }

    /// Freeze the weather. `true` bakes each planet's cloud deck once and reads
    /// it back per pixel; `false` (the default) evaluates it live, which is what
    /// the native generators do and the picture to A/B against. The web demo
    /// switches it on at construction.
    pub fn set_frozen_clouds(&mut self, on: bool) {
        self.frozen_clouds = on;
    }

    /// The outermost extent (world units) with the current view multipliers —
    /// handy for framing / zoom-fit.
    pub fn extent(&self) -> f32 {
        self.planets
            .last()
            // Aphelion (the far point) is a·(1 + e); frame against that.
            .map(|p| p.orbit * (1.0 + p.ecc(self.ecc)) * self.spacing + p.radius * self.planet_size)
            .unwrap_or(self.sun_radius * self.sun_size)
            + 40.0
    }
}

fn pick_kind(rng: &mut Rng, want_band: u8) -> usize {
    // Collect indices in the wanted band; fall back to all if none.
    let mut pool = [0usize; ROSTER.len()];
    let mut n = 0usize;
    for (i, a) in ROSTER.iter().enumerate() {
        if a.band == want_band {
            pool[n] = i;
            n += 1;
        }
    }
    if n == 0 {
        return (rng.f() * ROSTER.len() as f32) as usize % ROSTER.len();
    }
    pool[(rng.f() * n as f32) as usize % n]
}

// ===========================================================================
// Body tile renderers — each fills a small RGBA tile, transparent off-body.
// Both live in shared crates: the star in `sun-core`, the planets in
// `planet-core` (called straight from `draw_bodies`).
// ===========================================================================

/// Grid the star's clip rect is snapped out to, in tile px.
///
/// The clip is part of [`SunCache`]'s key, so a rect that slides a pixel as the
/// camera drifts would invalidate the cache every frame and undo the boil-clock
/// quantization. Snapping outward keeps it a superset of what `blit` reads while
/// holding it still for several frames of drift, at the cost of a border of
/// extra shading on a tile hundreds of px wide.
const SUN_CLIP_GRID: u32 = 32;

/// Round a visible-tile rect outward to [`SUN_CLIP_GRID`], clamped to the tile.
fn snap_out(r: [u32; 4], size: u32) -> [u32; 4] {
    let q = SUN_CLIP_GRID;
    [
        r[0] / q * q,
        r[1] / q * q,
        r[2].div_ceil(q).saturating_mul(q).min(size),
        r[3].div_ceil(q).saturating_mul(q).min(size),
    ]
}

/// Bake the star into `tile`, shading only `clip`. Pins solar's corona reach and
/// enables `sun-core`'s large-tile LOD (`true`).
fn render_sun_tile(tile: &mut Tile, sk: &SunKind, seed: u32, t: f32, rad_px: f32, clip: [u32; 4]) {
    sun_core::render_star_tile_into(tile, sk, seed, t, rad_px, CORONA_REACH, true, clip);
}

// ===========================================================================
// Scene compositor
// ===========================================================================

/// Low-saturation nebula tints; two are picked per system by seed.
const NEB_TINTS: &[Rgb] = &[
    [0.44, 0.20, 0.60], // violet
    [0.18, 0.36, 0.70], // blue
    [0.62, 0.24, 0.42], // rose
    [0.14, 0.52, 0.52], // teal
    [0.52, 0.34, 0.22], // dusty amber
    [0.30, 0.24, 0.68], // indigo
];

/// This system's sky: mostly pale/blue-white stars, a few warm, rare cyan.
const STAR_TINTS: StarTints = &[
    (0.46, [0.92, 0.95, 1.00]),
    (0.64, [0.72, 0.83, 1.00]),
    (0.78, [1.00, 0.96, 0.78]),
    (0.89, [1.00, 0.82, 0.60]),
    (0.96, [1.00, 0.62, 0.55]),
    (1.01, [0.72, 1.00, 0.95]),
];

/// Three parallax layers, each slower and dimmer than the last — and all slower
/// than the system itself, so a star can never appear to outrun a planet.
const STAR_LAYERS: &[StarLayer] = &[
    StarLayer { parallax: 0.13, spacing: 6.0, threshold: 0.80, brightness: 0.55, faint: 0.5, salt: 0 },
    StarLayer { parallax: 0.28, spacing: 8.0, threshold: 0.83, brightness: 0.80, faint: 0.5, salt: 1 },
    StarLayer { parallax: 0.45, spacing: 11.0, threshold: 0.86, brightness: 1.00, faint: 0.5, salt: 2 },
];

/// The deep-space ground: base navy under a faint seeded nebula, dithered into
/// pixel-art clouds. The nebula's own dither doubles as the ground's, which is
/// why `dither` here is 0.
const BACKDROP: Backdrop = Backdrop {
    base: [0.031, 0.027, 0.068],
    dither: 0.0,
    nebula: Some(Nebula {
        tints: NEB_TINTS,
        cell: 8,     // one fBm sample per 8x8 block -> pixel-art clouds
        quant: 2.0,  // snap the scroll: a small pan reuses the previous bake
        scroll: 0.09,
        strength: 0.34,
        dither: 0.015,
    }),
};

/// Paint the space background: a faint colored nebula plus parallax star layers.
///
/// The whole backdrop is anchored in SCREEN space — it scrolls on **pan** and does
/// not respond to **zoom** at all. Combined with zoom-about-centre (the JS wheel
/// and pinch keep `cam` fixed while zooming), the stars stay perfectly still as
/// you zoom, so they can never move faster than the solar system, and the
/// on-screen count stays constant: no wall when zoomed out, no swim.
///
/// `bgx`/`bgy` are the accumulated SCREEN-space pan of the camera (Δcam·zoom
/// summed over time), which is what makes each layer's rate constant at every
/// zoom. `density` scales the star count; the far layer and the nebula fade out
/// (and are skipped) once you zoom in on a body.
#[allow(clippy::too_many_arguments)]
fn paint_background(
    out: &mut [u8], w: u32, h: u32, cam: &Camera, seed: u32, density: f32, parallax: f32,
    bgx: f32, bgy: f32, cache: &RefCell<BackdropCache>,
) {
    let z = cam.zoom;
    let far_amt = 1.0 - smoothstep(3.0, 9.0, z);
    let neb_amt = 1.0 - smoothstep(2.5, 7.0, z);

    paint_backdrop(out, w, h, &BACKDROP, seed, bgx, bgy, parallax, neb_amt, Some(cache));

    let sky = Starfield {
        layers: STAR_LAYERS,
        tints: STAR_TINTS,
        density,
        pan_scale: parallax,
        far_fade: far_amt,
    };
    // Salt the star hash with the seed, so each system gets its own constellations.
    // Mixed into the hash's third axis rather than added to the cell coordinates:
    // offsetting the grid would give every system the SAME sky panned sideways,
    // which is the trap the nebula's plane offset used to fall into. The 977
    // stride clears the three layer salts, so one system's near layer can never
    // come out as the next system's far one.
    let sky_salt = (seed as i32).wrapping_mul(977);
    paint_stars(out, w, h, &sky, bgx, bgy, move |cx, cy, salt| {
        hash3(cx, cy, sky_salt.wrapping_add(17 + salt))
    });
}

/// Dot in a planet's orbit path as a faint dashed ellipse around the sun.
fn paint_orbit(out: &mut [u8], w: u32, h: u32, cam: &Camera, p: &Planet, spacing: f32, ecc: f32, width: f32) {
    let steps = 220;
    // Filled square stamp of half-extent `r`; width == 1 gives r == 0, i.e. the
    // original single-pixel dot (pixel-identical to the default look).
    let r = (((width - 1.0) * 0.5).round()) as i32;
    // Sample the ellipse by eccentric anomaly so the dashed path traces the exact
    // curve the planet travels (uniform in E draws the geometry, not the motion).
    let e = p.ecc(ecc);
    for k in 0..steps {
        // Dashed: skip every few samples.
        if (k / 3) % 2 == 0 {
            continue;
        }
        let ea = TAU * k as f32 / steps as f32;
        let (x1, y1) = p.plane_point(ea, e);
        let wx = x1 * spacing;
        let wy = y1 * ORBIT_FLATTEN * p.tilt * spacing;
        let (sx, sy) = to_screen(wx, wy, cam, w, h);
        let (px, py) = (sx as i32, sy as i32);
        for dy in -r..=r {
            for dx in -r..=r {
                let (x, y) = (px + dx, py + dy);
                if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 {
                    continue;
                }
                let idx = ((y as u32 * w + x as u32) * 4) as usize;
                // Additive faint blue-grey.
                out[idx] = (out[idx] as u32 + 26).min(90) as u8;
                out[idx + 1] = (out[idx + 1] as u32 + 30).min(96) as u8;
                out[idx + 2] = (out[idx + 2] as u32 + 40).min(120) as u8;
            }
        }
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
pub fn render_system(sys: &System, w: u32, h: u32, cam: &Camera, bgx: f32, bgy: f32, t_orbit: f32, t_spin: f32, t_sun: f32, out: &mut [u8]) {
    assert!(out.len() >= (w * h * 4) as usize);
    draw_bg_orbits(sys, w, h, cam, bgx, bgy, out);
    draw_bodies(sys, w, h, cam, t_orbit, t_spin, t_sun, out);
}

/// Cache key for the background + orbit layer: it's fully determined by the
/// camera + view params (NO animation time), so as long as these are unchanged
/// the backdrop is byte-for-byte identical frame to frame.
// The cached backdrop also paints the dashed orbit paths, which depend on the
// live orbit shape/weight — so eccentricity and orbit width are part of the key
// (alongside spacing) or the sliders would leave a stale backdrop.
type BgKey = [f32; 12];
fn bg_key(sys: &System, w: u32, h: u32, cam: &Camera, bgx: f32, bgy: f32) -> BgKey {
    [w as f32, h as f32, cam.x, cam.y, cam.zoom, sys.star_density, sys.star_parallax, sys.spacing, bgx, bgy, sys.ecc, sys.orbit_width]
}

/// Like [`render_system`] but caches the (time-independent) background + orbit
/// layer on the `System`. On a still camera — the common "watch it orbit" view —
/// the backdrop is a memcpy instead of a full re-render, which is >50% of the
/// frame. Any pan/zoom/view change invalidates the key and repaints once.
#[allow(clippy::too_many_arguments)]
pub fn render_system_cached(sys: &mut System, w: u32, h: u32, cam: &Camera, bgx: f32, bgy: f32, t_orbit: f32, t_spin: f32, t_sun: f32, out: &mut [u8]) {
    let len = (w * h * 4) as usize;
    assert!(out.len() >= len);
    let key = bg_key(sys, w, h, cam, bgx, bgy);
    if sys.bg_key == Some(key) && sys.bg_cache.len() == len {
        out[..len].copy_from_slice(&sys.bg_cache);
    } else {
        draw_bg_orbits(sys, w, h, cam, bgx, bgy, out);
        sys.bg_cache.clear();
        sys.bg_cache.extend_from_slice(&out[..len]);
        sys.bg_key = Some(key);
    }
    draw_bodies(sys, w, h, cam, t_orbit, t_spin, t_sun, out);
}

/// World position of planet `i` at time `t` — the same query `planet_pos` makes
/// over the C ABI, for callers on the Rust side (benchmarks, the native bins).
pub fn planet_pos_of(sys: &System, i: usize, t: f32) -> (f32, f32) {
    let p = &sys.planets[i];
    let (x, y, _) = p.at(t, sys.spacing, sys.ecc);
    (x, y)
}

/// Paint the backdrop: starfield + nebula, then the dashed orbit paths. Depends
/// only on the camera + view params, never on animation time.
fn draw_bg_orbits(sys: &System, w: u32, h: u32, cam: &Camera, bgx: f32, bgy: f32, out: &mut [u8]) {
    paint_background(out, w, h, cam, sys.seed, sys.star_density, sys.star_parallax, bgx, bgy, &sys.neb);
    for p in &sys.planets {
        paint_orbit(out, w, h, cam, p, sys.spacing, sys.ecc, sys.orbit_width);
    }
}

/// Draw the sun + planets over whatever is already in `out`, depth-sorted.
#[allow(clippy::too_many_arguments)]
fn draw_bodies(sys: &System, w: u32, h: u32, cam: &Camera, t_orbit: f32, t_spin: f32, t_sun: f32, out: &mut [u8]) {
    // Build a draw list of (depth, is_sun, planet_index) in the System's reused
    // scratch (no per-frame alloc). The sun sits at depth 0; planets sort around
    // it by their orbital depth.
    let mut order = sys.order.borrow_mut();
    order.clear();
    order.push((0.0, -1)); // sun
    for (i, p) in sys.planets.iter().enumerate() {
        let (_, _, depth) = p.at(t_orbit, sys.spacing, sys.ecc);
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

    for idx in 0..order.len() {
        let which = order[idx].1;
        if which < 0 {
            // The star. Per-body pixelation: render the tile smaller by
            // `sun_pixel`, then `blit` upsizes it by the same factor, so it
            // stays the same on-screen size but turns blockier.
            let rad_px = sys.sun_radius * sys.sun_size * cam.zoom;
            if rad_px < 0.5 {
                continue;
            }
            let rad_render = (rad_px / sys.sun_pixel).clamp(2.0, maxr_sun);
            let scale = rad_px / rad_render;
            // An empty rect is the visibility test; a partial one is shading
            // skipped where the star spills off the viewport.
            let tsize = sun_core::star_tile_size(rad_render, CORONA_REACH);
            let clip = visible_tile_rect(tsize, w, h, suncx, suncy, scale);
            if clip[2] == clip[0] {
                continue;
            }
            let clip = snap_out(clip, tsize); // hold the cache key still while panning
            // Re-bake at the QUANTIZED clock so the tile matches its key
            // exactly — same trick as the nebula field.
            let key = [
                rad_render.round() as i32,
                (t_sun / SUN_TQUANT).round() as i32,
                clip[0] as i32,
                clip[1] as i32,
                clip[2] as i32,
                clip[3] as i32,
            ];
            let mut sc = sys.sun_tile.borrow_mut();
            if sc.key != Some(key) {
                let tq = key[1] as f32 * SUN_TQUANT;
                render_sun_tile(&mut sc.tile, sun, sys.seed, tq, rad_render, clip);
                sc.key = Some(key);
            }
            blit(out, w, h, &sc.tile, suncx, suncy, scale);
        } else {
            let p = &sys.planets[which as usize];
            let (wx, wy, _depth) = p.at(t_orbit, sys.spacing, sys.ecc);
            let (sx, sy) = to_screen(wx, wy, cam, w, h);
            let rad_px = p.radius * sys.planet_size * cam.zoom;
            if rad_px < 0.5 {
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

            // `spin_a` both turns the surface and advances that world's weather —
            // the planet shader takes one angle for both, so a fast-spinning world
            // also churns faster.
            let spin_a = p.phase + p.spin * t_spin * TAU;
            let rad_render = (rad_px / sys.planet_pixel).clamp(2.0, maxr);
            let scale = rad_px / rad_render;
            // As for the star: the compositor decides what is worth shading.
            let tsize = planet_core::tile_size(p.ptype, rad_render);
            let clip = visible_tile_rect(tsize, w, h, sx, sy, scale);
            if clip[2] == clip[0] {
                continue;
            }
            // Freezing the weather is a per-scene switch, so the mask is built
            // here rather than baked into `render_tile`'s default.
            let feat = planet_core::F_ALL
                | if sys.frozen_clouds {
                    planet_core::F_NIGHT_LOD
                        | planet_core::F_BAKED_CLOUDS
                        | planet_core::F_BAKED_SURFACE
                        | planet_core::F_BAKED_BANDS
                        | planet_core::F_MORPH_LUT
                } else {
                    0
                };
            let mut tile = sys.body_tile.borrow_mut();
            planet_core::render_tile_into(&mut tile, p.ptype, p.seed, spin_a, light, rad_render, clip, feat);
            blit(out, w, h, &tile, sx, sy, scale);
        }
    }
}

/// World position of planet `i` at time `t` (for a camera that follows a body
/// as it orbits). Returns `(0, 0)` — the star — for an out-of-range index.
pub fn planet_world_pos(sys: &System, i: usize, t: f32) -> (f32, f32) {
    match sys.planets.get(i) {
        Some(p) => {
            let (x, y, _) = p.at(t, sys.spacing, sys.ecc);
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
        let (wx, wy, _) = p.at(t, sys.spacing, sys.ecc);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The roster names `planet-core` archetypes as strings, so a typo (or a
    /// renamed row over there) would silently fall back to type 0 and quietly
    /// fill systems with terrans. Pin it: every name resolves, and every
    /// archetype is placed exactly once.
    #[test]
    fn roster_covers_every_planet_type_once() {
        let mut seen = vec![0usize; planet_core::type_count()];
        for a in ROSTER {
            let i = planet_core::type_index(a.ty)
                .unwrap_or_else(|| panic!("no planet-core type named {:?}", a.ty));
            seen[i] += 1;
        }
        for (i, n) in seen.iter().enumerate() {
            assert_eq!(*n, 1, "{} appears {} times in ROSTER", planet_core::type_name(i), n);
        }
    }

    /// Every band must be non-empty, or `pick_kind` silently falls back to the
    /// whole table and the inner/outer character of a system washes out.
    #[test]
    fn every_orbital_band_has_worlds() {
        for band in 0..=2u8 {
            assert!(ROSTER.iter().any(|a| a.band == band), "orbital band {band} is empty");
        }
    }
}
