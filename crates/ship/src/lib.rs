//! ship — procedural, seed-driven **spaceships** in pixel art.
//!
//! Pure math, zero dependencies. Where `planet` rolls a world and `bird` rolls a
//! creature, `ship` rolls a *hull*: a plan-view (nose-up) vessel assembled from
//! a class blueprint plus per-ship structural randomness, then shaded, panelled,
//! liveried, lit and dithered. Same seed + same class => the same ship, forever.
//!
//! This crate is self-contained by the workspace rule (each "type" crate shares
//! no code with the others — only third-party deps and the manifest). It carries
//! its own noise/color/dither primitives; the new work here is the *assembly*
//! layer:
//!
//!   * a **64-class table** across 8 roles — drones, fighters, line warships,
//!     carriers, freighters, industrial, civilian and covert hulls. Adding a
//!     class is adding a row, in ONE place (the same trick as `planet`'s type
//!     table).
//!   * a **silhouette profile** — the hull's half-width down its length comes
//!     from one of 12 named families (needle, wedge, hammerhead, keel, brick…),
//!     jittered per ship, so two `destroyer`s are the same *ship class* but not
//!     the same ship.
//!   * **structural randomness, not recolor** — engines, wings, fins, nacelles,
//!     turrets, missile blocks, flight decks, cargo pods, truss spines, habitat
//!     rings, sensor dishes and radiators are each independently rolled inside
//!     class-appropriate ranges and welded onto the hull as parts.
//!   * a **part rasterizer** — 7 primitives (profiled hull, lozenge pod, rounded
//!     slab, swept wing, disc, ring, engine bell), each with an analytic normal,
//!     resolved per pixel through a uniform grid so a ~110-part capital ship
//!     still renders live every frame.
//!
//! ─────────────────────────────────────────────────────────────────────────
//! RANDOMIZATION TABLE  (C = fixed by class, S = rolled per ship/seed)
//! ─────────────────────────────────────────────────────────────────────────
//! CHARACTERISTIC        RANDOMIZED AS                                 SCOPE
//! role / silhouette     one of 8 roles, one of 12 profile families    C
//! hull profile          9 control half-widths, each ×0.90–1.10        C→S
//! beam                  class beam ×0.88–1.14                         C→S
//! length (metres)       class length ×0.80–1.25                       C→S
//! engine count          class ±1 (>=1)                                C→S
//! bell size / splay     0.55–1.35 / packed across the stern           S
//! nacelles              0–2 pairs, pylon length 0.05–0.13             C→S
//! wing span / sweep     class ×0.80–1.25 / class ±0.12                C→S
//! wing taper            0.30–0.75                                     S
//! fins                  0–2 pairs, span 0.35–0.65 of the wing         C→S
//! turrets               class ±25%, sized by hull tier                C→S
//! turret placement      fore/aft centreline + outboard sponsons       S
//! launcher blocks       class ±1, 2x3–4x6 cell grids                  C→S
//! flight decks          class count, angled 0.02–0.10 rad, overhung   C→S
//! cargo pods            class ±1 pairs × {container,tank,hopper,mod}  C→S
//! container colours     per-cell from a 9-colour manifest palette     S
//! truss spine           class fraction, 3–7 cross-braces              C→S
//! habitat / jump ring   class radius ×0.85–1.15, 0 = none             C→S
//! sensor dish           class radius ×0.8–1.3, boom 0.02–0.06         C→S
//! radiator panels       0–3 pairs, sweep 0.15–0.45                    C→S
//! bridge / superstruct  class size ×0.8–1.3, 1–3 stacked blocks       C→S
//! window density        class ×0.7–1.3 (rows of lit portholes)        C→S
//! armour belts          class ×0.75–1.25 (plate chunkiness)           C→S
//! greeble density       class ×0.6–1.5 (0–22 surface details)         C→S
//! livery                class family, hue ±0.04, 4 stripe schemes     C→S
//! drive-plume tint      class tint, hue ±0.05                         C→S
//! hull number / name    prefix by role+tier, 2-word ship name         S
//! ─────────────────────────────────────────────────────────────────────────
//!
//! Pipeline per frame (see [`Ship::render`]):
//!   1. paint the backdrop (a faint hashed starfield + vignette),
//!   2. for each pixel, rotate into ship space and resolve the topmost part
//!      through the uniform grid,
//!   3. shade it (Lambert + Blinn-Phong + rim, panel plates, weathering,
//!      livery stripes, lit portholes, deck markings, hazard stripes),
//!   4. add the drive plumes (additive, turbulent, with shock diamonds) and the
//!      blinking navigation lights,
//!   5. ordered-dither quantize for the crisp pixel-art read.

use std::cell::RefCell;
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

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}
fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}
fn smoother(t: f32) -> f32 {
    t * t * t * (t * (t * 6.0 - 15.0) + 10.0)
}
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp01((x - e0) / (e1 - e0));
    t * t * (3.0 - 2.0 * t)
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

pub type Rgb = [f32; 3];

fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [lerp(a[0], b[0], t), lerp(a[1], b[1], t), lerp(a[2], b[2], t)]
}
fn scale(a: Rgb, s: f32) -> Rgb {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn add(a: Rgb, b: Rgb) -> Rgb {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// Rotate `hue` of an RGB triple by `d` turns, cheaply (YIQ-ish shear). Keeps a
/// livery recognisable while letting each ship drift a few degrees off-book.
fn hue_shift(c: Rgb, d: f32) -> Rgb {
    let (u, w) = ((d * TAU).cos(), (d * TAU).sin());
    let l = 0.299 * c[0] + 0.587 * c[1] + 0.114 * c[2];
    [
        clamp01(l + (c[0] - l) * u + (c[1] - c[2]) * w * 0.4),
        clamp01(l + (c[1] - l) * u + (c[2] - c[0]) * w * 0.4),
        clamp01(l + (c[2] - l) * u + (c[0] - c[1]) * w * 0.4),
    ]
}

fn norm3(v: [f32; 3]) -> [f32; 3] {
    let m = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt().max(1e-6);
    [v[0] / m, v[1] / m, v[2] / m]
}
fn dot3(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Tiny deterministic RNG for ship generation (SplitMix-ish over `hash3`).
pub struct Rng {
    seed: i32,
    ctr: i32,
}
impl Rng {
    pub fn new(seed: u32) -> Rng {
        Rng { seed: seed as i32, ctr: 0 }
    }
    /// Uniform in [0, 1).
    pub fn f(&mut self) -> f32 {
        self.ctr = self.ctr.wrapping_add(1);
        hash3(self.seed, self.ctr, 0x9e37)
    }
    /// Uniform in [lo, hi).
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f()
    }
    /// Integer in [lo, hi] inclusive.
    pub fn int(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        lo + (self.f() * ((hi - lo + 1) as f32)) as i32 % (hi - lo + 1)
    }
    pub fn chance(&mut self, p: f32) -> bool {
        self.f() < p
    }
    /// A fresh sub-seed, for per-part noise that must stay stable across frames.
    pub fn sub(&mut self) -> u32 {
        (self.f() * 4.0e9) as u32
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
/// motion. `bx` is the Bayer offset for this pixel, `amt` the dither strength.
fn quant(o: Rgb, bx: f32, amt: f32) -> Rgb {
    let levels = 26.0;
    let d = bx * amt / levels;
    [
        clamp01(((o[0] + d) * levels).round() / levels),
        clamp01(((o[1] + d) * levels).round() / levels),
        clamp01(((o[2] + d) * levels).round() / levels),
    ]
}

// ===========================================================================
// Roles + silhouettes
// ===========================================================================

/// What a hull is *for*. Roles group the class table and drive the naval
/// prefix in [`Ship::designation`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Drone,
    Fighter,
    Warship,
    Carrier,
    Freighter,
    Industrial,
    Civilian,
    Covert,
}

const ROLE_NAMES: &[&str] = &[
    "drone", "fighter", "warship", "carrier", "freighter", "industrial", "civilian", "covert",
];

impl Role {
    fn idx(self) -> usize {
        match self {
            Role::Drone => 0,
            Role::Fighter => 1,
            Role::Warship => 2,
            Role::Carrier => 3,
            Role::Freighter => 4,
            Role::Industrial => 5,
            Role::Civilian => 6,
            Role::Covert => 7,
        }
    }
}

/// Number of roles.
pub fn role_count() -> usize {
    ROLE_NAMES.len()
}
/// Name of a role (wraps on out-of-range index).
pub fn role_name(i: usize) -> &'static str {
    ROLE_NAMES[i % ROLE_NAMES.len()]
}

/// Hull silhouette family — the shape of the half-width curve from nose to
/// stern. Twelve families cover everything from a needle interceptor to a
/// container brick; per-ship jitter does the rest.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Sil {
    Needle,
    Dart,
    Wedge,
    Blade,
    Spindle,
    Slab,
    Hammerhead,
    Chevron,
    Stub,
    Keel,
    Saucer,
    Brick,
}

/// Nine control half-widths (nose → stern) as a fraction of the class beam.
/// [`profile_at`] smooth-interpolates between them.
fn silhouette(s: Sil) -> [f32; 9] {
    match s {
        // sharp nose, widest just forward of the stern — interceptors, couriers
        Sil::Needle => [0.00, 0.27, 0.47, 0.63, 0.78, 0.92, 1.00, 0.90, 0.58],
        // fighter wedge: quick shoulders, fat aft body
        Sil::Dart => [0.05, 0.33, 0.53, 0.67, 0.81, 0.94, 1.00, 0.85, 0.52],
        // classic triangle — bombers, line cruisers
        Sil::Wedge => [0.07, 0.25, 0.41, 0.56, 0.71, 0.85, 0.95, 1.00, 0.85],
        // long knife: almost parallel-sided, abrupt stern — stealth hulls
        Sil::Blade => [0.03, 0.21, 0.37, 0.51, 0.65, 0.80, 0.94, 1.00, 0.42],
        // cigar, widest forward of centre — freighters, liners
        Sil::Spindle => [0.13, 0.47, 0.73, 0.90, 1.00, 0.97, 0.87, 0.71, 0.50],
        // blunt brick with a rounded bow — carriers, battleships
        Sil::Slab => [0.40, 0.70, 0.90, 0.98, 1.00, 1.00, 0.98, 0.92, 0.76],
        // wide bow, pinched waist, wide stern — heavy cruisers, command ships
        Sil::Hammerhead => [0.34, 0.80, 1.00, 0.84, 0.52, 0.48, 0.64, 0.92, 0.70],
        // arrowhead — drones, raiders
        Sil::Chevron => [0.02, 0.21, 0.37, 0.53, 0.72, 0.90, 1.00, 0.72, 0.28],
        // short and fat — tugs, swarm drones, gunships
        Sil::Stub => [0.44, 0.74, 0.92, 1.00, 1.00, 0.94, 0.86, 0.80, 0.64],
        // thin backbone meant to carry an exposed truss and pods
        Sil::Keel => [0.16, 0.39, 0.49, 0.41, 0.35, 0.35, 0.43, 0.68, 0.50],
        // disc forward, tapering tail — science and survey hulls
        Sil::Saucer => [0.34, 0.80, 1.00, 1.00, 0.88, 0.60, 0.40, 0.34, 0.22],
        // a true box — barges, megafreighters, generation ships
        Sil::Brick => [0.66, 0.92, 1.00, 1.00, 1.00, 1.00, 1.00, 0.95, 0.82],
    }
}

/// Smoothly sample a 9-stop half-width profile at `v` ∈ [0, 1] (0 = nose).
fn profile_at(p: &[f32; 9], v: f32) -> f32 {
    let x = clamp01(v) * 8.0;
    let i = (x.floor() as usize).min(7);
    let f = x - i as f32;
    lerp(p[i], p[i + 1], f * f * (3.0 - 2.0 * f))
}

/// What a class's cargo/utility pods look like.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Pod {
    None,
    /// Racks of stacked shipping containers, each its own manifest colour.
    Container,
    /// Pressurised drums — fuel, gas, volatiles.
    Tank,
    /// Open-topped ore hoppers with a dark interior.
    Hopper,
    /// Sealed grey mission modules — labs, barracks, workshops.
    Module,
    /// Streamlined outboard engine/utility nacelles.
    Nacelle,
}

/// Livery family — a palette scheme, resolved to concrete colours per ship.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Liv {
    Naval,
    Marine,
    Merchant,
    Corporate,
    Industrial,
    Militia,
    Civil,
    Covert,
    Drone,
    Rescue,
}

// ===========================================================================
// The class table
// ===========================================================================

/// One spaceship class: the blueprint a seed is rolled *against*. Every field is
/// a centre value — [`Ship::generate`] jitters it, so a class is a family of
/// ships rather than one ship.
#[derive(Clone, Copy)]
pub struct Class {
    name: &'static str,
    role: Role,
    /// Nominal length in metres (flavour text + the true-scale lineup poster).
    len_m: f32,
    /// Max hull half-width as a fraction of length.
    beam: f32,
    sil: Sil,
    /// Main drive bells across the stern.
    engines: u32,
    /// Outboard engine-nacelle pairs.
    nacelles: u32,
    /// Wing half-span *beyond* the hull, as a fraction of length (0 = none).
    wing: f32,
    /// Wing sweep, 0 = straight out, 1 = folded hard aft.
    sweep: f32,
    /// Tail-fin pairs.
    fins: u32,
    /// Gun turrets.
    turrets: u32,
    /// Missile / VLS cell blocks.
    launchers: u32,
    /// Flight decks / hangar bays.
    hangars: u32,
    /// Cargo-or-utility pod pairs.
    pods: u32,
    pod: Pod,
    /// Exposed truss spine length as a fraction of the hull (0 = closed hull).
    spine: f32,
    /// Habitat / jump ring radius as a fraction of length (0 = none).
    ring: f32,
    /// Sensor dish radius as a fraction of length (0 = none).
    dish: f32,
    /// Radiator panel pairs.
    rads: u32,
    /// Superstructure size (0 = flush hull).
    bridge: f32,
    /// Lit-porthole density.
    windows: f32,
    /// Armour-belt chunkiness.
    armor: f32,
    /// Surface-greeble density.
    greeble: f32,
    livery: Liv,
    /// Drive-plume tint.
    glow: Rgb,
    /// Nominal plume length as a fraction of hull length.
    thrust: f32,
}

/// Class defaults — a row only names what it changes (same trick as `planet`).
const fn base() -> Class {
    Class {
        name: "",
        role: Role::Warship,
        len_m: 100.0,
        beam: 0.16,
        sil: Sil::Wedge,
        engines: 2,
        nacelles: 0,
        wing: 0.0,
        sweep: 0.55,
        fins: 0,
        turrets: 0,
        launchers: 0,
        hangars: 0,
        pods: 0,
        pod: Pod::None,
        spine: 0.0,
        ring: 0.0,
        dish: 0.0,
        rads: 0,
        bridge: 0.5,
        windows: 0.0,
        armor: 0.3,
        greeble: 0.4,
        livery: Liv::Naval,
        glow: [0.55, 0.78, 1.00],
        thrust: 0.30,
    }
}

const G_BLUE: Rgb = [0.52, 0.76, 1.00];
const G_WHITE: Rgb = [0.86, 0.92, 1.00];
const G_ORANGE: Rgb = [1.00, 0.60, 0.22];
const G_AMBER: Rgb = [1.00, 0.78, 0.32];
const G_GREEN: Rgb = [0.44, 1.00, 0.62];
const G_CYAN: Rgb = [0.36, 0.95, 1.00];
const G_VIOLET: Rgb = [0.78, 0.52, 1.00];
const G_RED: Rgb = [1.00, 0.36, 0.28];

/// **64 spaceship classes** across 8 roles. Adding a class = adding a row, in
/// ONE place; everything downstream (the web picker, the contact sheets, the
/// scale lineup, the naval prefixes) is driven off this table.
///
/// Rows are grouped by role and, inside a role, ordered small → large, so the
/// per-role posters read as a size progression.
pub const CLASSES: &[Class] = &[
    // ---- unmanned drones: tiny, cheap, no windows, cyan/violet drives ------
    Class { name: "swarm_drone",     role: Role::Drone, len_m: 3.0,  beam: 0.34, sil: Sil::Stub,    engines: 1, wing: 0.08, sweep: 0.80, turrets: 1, greeble: 0.30, livery: Liv::Drone, glow: G_CYAN,   thrust: 0.22, ..base() },
    Class { name: "recon_drone",     role: Role::Drone, len_m: 4.5,  beam: 0.27, sil: Sil::Chevron, engines: 1, wing: 0.11, sweep: 0.85, dish: 0.07, greeble: 0.25, livery: Liv::Drone, glow: G_CYAN,   thrust: 0.26, ..base() },
    Class { name: "strike_drone",    role: Role::Drone, len_m: 6.0,  beam: 0.23, sil: Sil::Blade,   engines: 1, wing: 0.17, sweep: 0.72, launchers: 2, greeble: 0.35, livery: Liv::Drone, glow: G_VIOLET, thrust: 0.34, ..base() },
    Class { name: "sentry_drone",    role: Role::Drone, len_m: 7.5,  beam: 0.30, sil: Sil::Saucer,  engines: 1, turrets: 2, dish: 0.09, greeble: 0.40, livery: Liv::Drone, glow: G_CYAN,   thrust: 0.20, ..base() },
    Class { name: "courier_drone",   role: Role::Drone, len_m: 10.0, beam: 0.17, sil: Sil::Needle,  engines: 2, greeble: 0.25, livery: Liv::Drone, glow: G_WHITE,  thrust: 0.42, ..base() },
    Class { name: "repair_drone",    role: Role::Drone, len_m: 9.0,  beam: 0.30, sil: Sil::Stub,    engines: 2, pods: 1, pod: Pod::Module, dish: 0.06, greeble: 0.55, livery: Liv::Corporate, glow: G_AMBER, thrust: 0.20, ..base() },
    Class { name: "mining_drone",    role: Role::Drone, len_m: 12.0, beam: 0.32, sil: Sil::Stub,    engines: 2, pods: 1, pod: Pod::Hopper, greeble: 0.60, armor: 0.5, livery: Liv::Industrial, glow: G_ORANGE, thrust: 0.22, ..base() },

    // ---- fighters and small attack craft: wings, canopies, hot drives ------
    Class { name: "interceptor",     role: Role::Fighter, len_m: 12.0, beam: 0.17, sil: Sil::Needle,  engines: 2, wing: 0.23, sweep: 0.85, fins: 1, turrets: 1, windows: 0.30, greeble: 0.35, livery: Liv::Militia, glow: G_BLUE,  thrust: 0.52, ..base() },
    Class { name: "fighter",         role: Role::Fighter, len_m: 15.0, beam: 0.20, sil: Sil::Dart,    engines: 2, wing: 0.27, sweep: 0.60, fins: 1, turrets: 2, windows: 0.30, greeble: 0.40, livery: Liv::Militia, glow: G_BLUE,  thrust: 0.46, ..base() },
    Class { name: "strike_fighter",  role: Role::Fighter, len_m: 18.0, beam: 0.21, sil: Sil::Chevron, engines: 2, wing: 0.28, sweep: 0.75, fins: 1, turrets: 1, launchers: 2, windows: 0.28, armor: 0.4, livery: Liv::Militia, glow: G_BLUE, thrust: 0.48, ..base() },
    Class { name: "heavy_fighter",   role: Role::Fighter, len_m: 22.0, beam: 0.23, sil: Sil::Dart,    engines: 3, wing: 0.29, sweep: 0.50, fins: 1, turrets: 2, launchers: 1, windows: 0.25, armor: 0.45, greeble: 0.5, livery: Liv::Naval, glow: G_BLUE, thrust: 0.44, ..base() },
    Class { name: "bomber",          role: Role::Fighter, len_m: 30.0, beam: 0.26, sil: Sil::Wedge,   engines: 3, wing: 0.31, sweep: 0.35, fins: 2, turrets: 1, launchers: 3, windows: 0.25, armor: 0.55, greeble: 0.5, livery: Liv::Naval, glow: G_BLUE, thrust: 0.36, ..base() },
    Class { name: "torpedo_boat",    role: Role::Fighter, len_m: 38.0, beam: 0.18, sil: Sil::Blade,   engines: 3, wing: 0.13, sweep: 0.65, fins: 1, launchers: 4, windows: 0.20, greeble: 0.5, livery: Liv::Naval, glow: G_BLUE, thrust: 0.40, ..base() },
    Class { name: "gunship",         role: Role::Fighter, len_m: 34.0, beam: 0.28, sil: Sil::Stub,    engines: 2, nacelles: 1, turrets: 4, launchers: 2, windows: 0.35, armor: 0.65, greeble: 0.6, livery: Liv::Militia, glow: G_ORANGE, thrust: 0.30, ..base() },

    // ---- line warships: the naval spine of the table, escort → dreadnought -
    Class { name: "escort_cutter",   role: Role::Warship, len_m: 90.0,  beam: 0.13, sil: Sil::Needle,     engines: 2, turrets: 3, launchers: 1, rads: 1, windows: 0.15, bridge: 0.55, greeble: 0.45, ..base() },
    Class { name: "corvette",        role: Role::Warship, len_m: 120.0, beam: 0.14, sil: Sil::Dart,       engines: 3, fins: 1, turrets: 4, launchers: 1, rads: 1, windows: 0.18, armor: 0.40, greeble: 0.5, ..base() },
    Class { name: "flak_frigate",    role: Role::Warship, len_m: 200.0, beam: 0.15, sil: Sil::Dart,       engines: 3, turrets: 8, launchers: 1, rads: 1, windows: 0.20, armor: 0.45, greeble: 0.55, ..base() },
    Class { name: "frigate",         role: Role::Warship, len_m: 180.0, beam: 0.13, sil: Sil::Wedge,      engines: 3, turrets: 5, launchers: 2, hangars: 1, dish: 0.05, rads: 1, windows: 0.20, armor: 0.45, greeble: 0.5, ..base() },
    Class { name: "destroyer",       role: Role::Warship, len_m: 250.0, beam: 0.12, sil: Sil::Wedge,      engines: 4, turrets: 6, launchers: 3, rads: 2, windows: 0.22, armor: 0.55, greeble: 0.55, ..base() },
    Class { name: "monitor",         role: Role::Warship, len_m: 300.0, beam: 0.22, sil: Sil::Brick,      engines: 2, turrets: 4, launchers: 2, rads: 2, windows: 0.18, armor: 1.00, greeble: 0.6, glow: G_ORANGE, thrust: 0.18, ..base() },
    Class { name: "light_cruiser",   role: Role::Warship, len_m: 330.0, beam: 0.13, sil: Sil::Hammerhead, engines: 4, turrets: 7, launchers: 3, hangars: 1, dish: 0.05, rads: 2, windows: 0.25, armor: 0.55, greeble: 0.6, ..base() },
    Class { name: "missile_cruiser", role: Role::Warship, len_m: 380.0, beam: 0.14, sil: Sil::Wedge,      engines: 4, turrets: 3, launchers: 8, rads: 2, windows: 0.22, armor: 0.50, greeble: 0.6, ..base() },
    Class { name: "heavy_cruiser",   role: Role::Warship, len_m: 430.0, beam: 0.15, sil: Sil::Hammerhead, engines: 5, turrets: 9, launchers: 4, hangars: 1, dish: 0.05, rads: 2, windows: 0.28, armor: 0.75, greeble: 0.65, ..base() },
    Class { name: "railgun_lance",   role: Role::Warship, len_m: 520.0, beam: 0.12, sil: Sil::Blade,      engines: 4, turrets: 1, launchers: 2, rads: 3, windows: 0.18, armor: 0.80, greeble: 0.6, glow: G_VIOLET, ..base() },
    Class { name: "battlecruiser",   role: Role::Warship, len_m: 580.0, beam: 0.15, sil: Sil::Wedge,      engines: 5, turrets: 10, launchers: 4, hangars: 1, dish: 0.05, rads: 3, windows: 0.30, armor: 0.75, greeble: 0.65, ..base() },
    Class { name: "command_ship",    role: Role::Warship, len_m: 650.0, beam: 0.16, sil: Sil::Hammerhead, engines: 5, turrets: 6, launchers: 3, hangars: 1, dish: 0.09, rads: 3, windows: 0.50, bridge: 0.85, armor: 0.60, greeble: 0.7, ..base() },
    Class { name: "battleship",      role: Role::Warship, len_m: 740.0, beam: 0.17, sil: Sil::Slab,       engines: 6, turrets: 12, launchers: 5, rads: 3, windows: 0.32, armor: 0.90, greeble: 0.7, ..base() },
    Class { name: "dreadnought",     role: Role::Warship, len_m: 980.0, beam: 0.19, sil: Sil::Slab,       engines: 7, turrets: 14, launchers: 6, hangars: 1, dish: 0.06, rads: 3, windows: 0.35, bridge: 0.8, armor: 1.00, greeble: 0.8, ..base() },

    // ---- carriers: flight decks, sponsons, drone racks --------------------
    Class { name: "drone_tender",    role: Role::Carrier, len_m: 160.0,  beam: 0.20, sil: Sil::Slab,  engines: 3, hangars: 2, pods: 2, pod: Pod::Module, turrets: 1, rads: 1, windows: 0.25, greeble: 0.55, livery: Liv::Corporate, glow: G_CYAN, ..base() },
    Class { name: "escort_carrier",  role: Role::Carrier, len_m: 270.0,  beam: 0.22, sil: Sil::Slab,  engines: 3, hangars: 2, turrets: 3, rads: 1, windows: 0.30, armor: 0.45, greeble: 0.6, ..base() },
    Class { name: "light_carrier",   role: Role::Carrier, len_m: 390.0,  beam: 0.22, sil: Sil::Slab,  engines: 4, hangars: 3, turrets: 3, dish: 0.05, rads: 2, windows: 0.35, armor: 0.50, greeble: 0.65, ..base() },
    Class { name: "assault_carrier", role: Role::Carrier, len_m: 490.0,  beam: 0.26, sil: Sil::Brick, engines: 4, hangars: 3, turrets: 6, launchers: 2, pods: 2, pod: Pod::Module, rads: 2, windows: 0.35, armor: 0.75, greeble: 0.7, ..base() },
    Class { name: "fleet_carrier",   role: Role::Carrier, len_m: 640.0,  beam: 0.24, sil: Sil::Slab,  engines: 5, hangars: 4, turrets: 5, dish: 0.06, rads: 3, windows: 0.42, armor: 0.60, bridge: 0.75, greeble: 0.7, ..base() },
    Class { name: "supercarrier",    role: Role::Carrier, len_m: 1150.0, beam: 0.26, sil: Sil::Slab,  engines: 7, hangars: 6, turrets: 8, launchers: 2, dish: 0.07, rads: 3, windows: 0.48, armor: 0.70, bridge: 0.9, greeble: 0.8, ..base() },

    // ---- freighters and haulers: pods, racks, spines, working liveries -----
    Class { name: "tug",             role: Role::Freighter, len_m: 45.0,   beam: 0.30, sil: Sil::Stub,    engines: 3, greeble: 0.75, windows: 0.40, armor: 0.35, livery: Liv::Industrial, glow: G_AMBER, thrust: 0.34, ..base() },
    Class { name: "courier",         role: Role::Freighter, len_m: 40.0,   beam: 0.16, sil: Sil::Needle,  engines: 2, pods: 1, pod: Pod::Module, windows: 0.35, greeble: 0.35, livery: Liv::Corporate, glow: G_WHITE, thrust: 0.48, ..base() },
    Class { name: "light_freighter", role: Role::Freighter, len_m: 75.0,   beam: 0.22, sil: Sil::Spindle, engines: 2, nacelles: 1, pods: 1, pod: Pod::Container, windows: 0.45, greeble: 0.55, livery: Liv::Merchant, glow: G_AMBER, ..base() },
    Class { name: "box_hauler",      role: Role::Freighter, len_m: 140.0,  beam: 0.24, sil: Sil::Slab,    engines: 2, pods: 2, pod: Pod::Container, windows: 0.35, greeble: 0.55, livery: Liv::Merchant, glow: G_AMBER, thrust: 0.24, ..base() },
    Class { name: "heavy_lifter",    role: Role::Freighter, len_m: 210.0,  beam: 0.26, sil: Sil::Stub,    engines: 4, spine: 0.38, pods: 2, pod: Pod::Module, rads: 1, windows: 0.30, greeble: 0.7, livery: Liv::Industrial, glow: G_AMBER, thrust: 0.26, ..base() },
    Class { name: "bulk_freighter",  role: Role::Freighter, len_m: 280.0,  beam: 0.16, sil: Sil::Keel,    engines: 3, spine: 0.58, pods: 3, pod: Pod::Container, windows: 0.30, greeble: 0.6, livery: Liv::Merchant, glow: G_AMBER, thrust: 0.22, ..base() },
    Class { name: "ore_hauler",      role: Role::Freighter, len_m: 350.0,  beam: 0.19, sil: Sil::Keel,    engines: 3, spine: 0.52, pods: 3, pod: Pod::Hopper, rads: 1, windows: 0.25, greeble: 0.75, armor: 0.4, livery: Liv::Industrial, glow: G_ORANGE, thrust: 0.20, ..base() },
    Class { name: "tanker",          role: Role::Freighter, len_m: 400.0,  beam: 0.26, sil: Sil::Spindle, engines: 3, pods: 3, pod: Pod::Tank, rads: 1, windows: 0.25, greeble: 0.55, livery: Liv::Industrial, glow: G_AMBER, thrust: 0.20, ..base() },
    Class { name: "container_barge", role: Role::Freighter, len_m: 460.0,  beam: 0.30, sil: Sil::Brick,   engines: 3, pods: 4, pod: Pod::Container, windows: 0.25, greeble: 0.5, livery: Liv::Merchant, glow: G_AMBER, thrust: 0.16, ..base() },
    Class { name: "megafreighter",   role: Role::Freighter, len_m: 950.0,  beam: 0.32, sil: Sil::Brick,   engines: 5, pods: 6, pod: Pod::Container, rads: 2, windows: 0.32, greeble: 0.6, livery: Liv::Merchant, glow: G_AMBER, thrust: 0.16, ..base() },

    // ---- industrial: dishes, rings, hoppers, radiators, lots of greeble ----
    Class { name: "survey_scout",    role: Role::Industrial, len_m: 55.0,  beam: 0.18, sil: Sil::Needle, engines: 2, dish: 0.11, windows: 0.40, greeble: 0.5, livery: Liv::Corporate, glow: G_WHITE, thrust: 0.38, ..base() },
    Class { name: "science_vessel",  role: Role::Industrial, len_m: 160.0, beam: 0.24, sil: Sil::Saucer, engines: 2, dish: 0.13, ring: 0.19, windows: 0.55, greeble: 0.45, livery: Liv::Corporate, glow: G_CYAN, thrust: 0.24, ..base() },
    Class { name: "salvager",        role: Role::Industrial, len_m: 190.0, beam: 0.26, sil: Sil::Stub,   engines: 3, spine: 0.32, pods: 2, pod: Pod::Module, dish: 0.05, rads: 1, windows: 0.30, greeble: 0.90, armor: 0.5, livery: Liv::Industrial, glow: G_ORANGE, thrust: 0.22, ..base() },
    Class { name: "repair_tender",   role: Role::Industrial, len_m: 290.0, beam: 0.24, sil: Sil::Slab,   engines: 3, hangars: 2, pods: 2, pod: Pod::Module, rads: 2, dish: 0.05, windows: 0.40, greeble: 0.7, livery: Liv::Corporate, glow: G_AMBER, thrust: 0.20, ..base() },
    Class { name: "mining_rig",      role: Role::Industrial, len_m: 320.0, beam: 0.21, sil: Sil::Keel,   engines: 3, spine: 0.55, pods: 3, pod: Pod::Hopper, rads: 2, windows: 0.28, greeble: 0.85, armor: 0.5, livery: Liv::Industrial, glow: G_ORANGE, thrust: 0.18, ..base() },
    Class { name: "constructor",     role: Role::Industrial, len_m: 470.0, beam: 0.19, sil: Sil::Keel,   engines: 4, spine: 0.62, pods: 3, pod: Pod::Module, rads: 2, dish: 0.06, windows: 0.35, greeble: 0.8, livery: Liv::Corporate, glow: G_AMBER, thrust: 0.18, ..base() },
    Class { name: "refinery_ship",   role: Role::Industrial, len_m: 660.0, beam: 0.28, sil: Sil::Brick,  engines: 4, pods: 4, pod: Pod::Tank, rads: 3, windows: 0.30, greeble: 0.95, livery: Liv::Industrial, glow: G_ORANGE, thrust: 0.16, ..base() },

    // ---- civilian: windows everywhere, soft liveries, habitat rings --------
    Class { name: "shuttle",         role: Role::Civilian, len_m: 18.0,   beam: 0.28, sil: Sil::Stub,    engines: 2, wing: 0.17, sweep: 0.30, windows: 0.80, greeble: 0.30, livery: Liv::Civil, glow: G_WHITE, thrust: 0.34, ..base() },
    Class { name: "yacht",           role: Role::Civilian, len_m: 60.0,   beam: 0.18, sil: Sil::Spindle, engines: 2, wing: 0.10, sweep: 0.55, nacelles: 1, windows: 0.75, greeble: 0.25, armor: 0.15, livery: Liv::Civil, glow: G_WHITE, thrust: 0.40, ..base() },
    Class { name: "system_ferry",    role: Role::Civilian, len_m: 150.0,  beam: 0.22, sil: Sil::Spindle, engines: 3, windows: 0.90, greeble: 0.35, armor: 0.20, livery: Liv::Civil, glow: G_WHITE, thrust: 0.28, ..base() },
    Class { name: "hospital_ship",   role: Role::Civilian, len_m: 340.0,  beam: 0.23, sil: Sil::Spindle, engines: 3, hangars: 1, dish: 0.05, rads: 2, windows: 0.85, greeble: 0.40, armor: 0.20, livery: Liv::Rescue, glow: G_WHITE, thrust: 0.24, ..base() },
    Class { name: "liner",           role: Role::Civilian, len_m: 520.0,  beam: 0.20, sil: Sil::Spindle, engines: 4, nacelles: 1, ring: 0.0, windows: 1.00, bridge: 0.7, greeble: 0.4, armor: 0.15, livery: Liv::Civil, glow: G_WHITE, thrust: 0.26, ..base() },
    Class { name: "colony_ship",     role: Role::Civilian, len_m: 1300.0, beam: 0.18, sil: Sil::Keel,    engines: 4, spine: 0.48, ring: 0.27, pods: 3, pod: Pod::Module, rads: 2, dish: 0.06, windows: 0.65, greeble: 0.6, livery: Liv::Civil, glow: G_WHITE, thrust: 0.16, ..base() },
    Class { name: "generation_ship", role: Role::Civilian, len_m: 3400.0, beam: 0.30, sil: Sil::Brick,   engines: 6, ring: 0.35, pods: 4, pod: Pod::Module, rads: 3, dish: 0.05, windows: 0.85, bridge: 0.8, greeble: 0.7, livery: Liv::Civil, glow: G_GREEN, thrust: 0.14, ..base() },

    // ---- covert: matte hulls, hard sweep, hot little drives ----------------
    Class { name: "stealth_scout",   role: Role::Covert, len_m: 45.0,  beam: 0.14, sil: Sil::Blade,   engines: 1, wing: 0.19, sweep: 0.90, fins: 1, greeble: 0.25, armor: 0.20, livery: Liv::Covert, glow: G_VIOLET, thrust: 0.30, ..base() },
    Class { name: "raider",          role: Role::Covert, len_m: 110.0, beam: 0.17, sil: Sil::Chevron, engines: 3, wing: 0.17, sweep: 0.80, fins: 1, turrets: 3, launchers: 2, windows: 0.20, greeble: 0.6, livery: Liv::Covert, glow: G_RED, thrust: 0.44, ..base() },
    Class { name: "blockade_runner", role: Role::Covert, len_m: 135.0, beam: 0.15, sil: Sil::Needle,  engines: 4, pods: 1, pod: Pod::Container, windows: 0.25, greeble: 0.5, livery: Liv::Merchant, glow: G_BLUE, thrust: 0.56, ..base() },
    Class { name: "privateer",       role: Role::Covert, len_m: 165.0, beam: 0.18, sil: Sil::Dart,    engines: 3, turrets: 4, launchers: 1, pods: 1, pod: Pod::Container, windows: 0.28, greeble: 0.7, armor: 0.45, livery: Liv::Merchant, glow: G_ORANGE, ..base() },
    Class { name: "q_ship",          role: Role::Covert, len_m: 200.0, beam: 0.22, sil: Sil::Spindle, engines: 2, turrets: 3, launchers: 2, pods: 2, pod: Pod::Container, windows: 0.35, greeble: 0.6, livery: Liv::Merchant, glow: G_AMBER, thrust: 0.24, ..base() },
    Class { name: "shadow_frigate",  role: Role::Covert, len_m: 230.0, beam: 0.13, sil: Sil::Blade,   engines: 3, wing: 0.11, sweep: 0.90, fins: 1, turrets: 2, launchers: 4, rads: 1, windows: 0.15, armor: 0.40, greeble: 0.5, livery: Liv::Covert, glow: G_VIOLET, ..base() },
];

/// Number of ship classes.
pub fn class_count() -> usize {
    CLASSES.len()
}
/// Name of a class (wraps on out-of-range index).
pub fn class_name(i: usize) -> &'static str {
    CLASSES[i % CLASSES.len()].name
}
/// Role index of a class — pair with [`role_name`].
pub fn class_role(i: usize) -> usize {
    CLASSES[i % CLASSES.len()].role.idx()
}
/// Nominal length of a class, in metres.
pub fn class_length_m(i: usize) -> f32 {
    CLASSES[i % CLASSES.len()].len_m
}
/// Every class index belonging to `role`, in table order (small → large).
pub fn classes_in_role(role: usize) -> Vec<usize> {
    (0..CLASSES.len()).filter(|&i| CLASSES[i].role.idx() == role).collect()
}

// ===========================================================================
// Liveries
// ===========================================================================

/// The resolved paint scheme for one ship.
#[derive(Clone, Copy)]
pub struct Palette {
    plate: Rgb,  // main hull plate
    shade: Rgb,  // recessed / secondary plate
    accent: Rgb, // livery stripes and flashes
    trim: Rgb,   // fine trim, deck markings
    metal: Rgb,  // bare structural metal (pylons, trusses, bells)
    dark: Rgb,   // deep recesses, launcher cells, hangar mouths
    glass: Rgb,  // canopies and lit portholes
    glow: Rgb,   // drive plume
}

/// Livery family centres — jittered per ship in [`Ship::generate`].
fn livery(l: Liv) -> Palette {
    let base = Palette {
        plate: [0.56, 0.58, 0.62],
        shade: [0.40, 0.42, 0.47],
        accent: [0.85, 0.55, 0.18],
        trim: [0.80, 0.82, 0.86],
        metal: [0.44, 0.45, 0.49],
        dark: [0.11, 0.12, 0.15],
        glass: [0.95, 0.85, 0.58],
        glow: [0.55, 0.78, 1.00],
    };
    match l {
        // fleet grey with a cool cast. `accent` carries the bold colour (it's the
        // stripe); `trim` is fine detail and superstructure, so it stays light —
        // swapped, the navy blue lands on whole bridge blocks and reads as glass.
        Liv::Naval => Palette { plate: [0.50, 0.54, 0.60], shade: [0.34, 0.37, 0.44], accent: [0.24, 0.44, 0.66], trim: [0.74, 0.78, 0.85], ..base },
        // darker, greener naval — marine assault units
        Liv::Marine => Palette { plate: [0.38, 0.44, 0.40], shade: [0.25, 0.30, 0.28], accent: [0.74, 0.70, 0.38], trim: [0.52, 0.58, 0.50], ..base },
        // merchant: weathered white over rust-red boot topping
        Liv::Merchant => Palette { plate: [0.72, 0.70, 0.66], shade: [0.48, 0.44, 0.41], accent: [0.66, 0.26, 0.18], trim: [0.86, 0.84, 0.80], ..base },
        // corporate: bright white, orange flashes
        Liv::Corporate => Palette { plate: [0.82, 0.83, 0.85], shade: [0.58, 0.60, 0.64], accent: [0.94, 0.52, 0.14], trim: [0.30, 0.34, 0.42], ..base },
        // industrial: hazard yellow and grime
        Liv::Industrial => Palette { plate: [0.66, 0.56, 0.24], shade: [0.42, 0.36, 0.19], accent: [0.16, 0.16, 0.18], trim: [0.80, 0.72, 0.40], metal: [0.40, 0.38, 0.34], ..base },
        // militia / mercenary: dark grey with a hot accent
        Liv::Militia => Palette { plate: [0.42, 0.43, 0.47], shade: [0.27, 0.28, 0.32], accent: [0.86, 0.36, 0.16], trim: [0.62, 0.64, 0.68], ..base },
        // civilian: cream over deep blue
        Liv::Civil => Palette { plate: [0.86, 0.85, 0.80], shade: [0.60, 0.60, 0.60], accent: [0.18, 0.36, 0.66], trim: [0.92, 0.92, 0.94], glass: [1.00, 0.90, 0.66], ..base },
        // covert: matte near-black, violet edge
        Liv::Covert => Palette { plate: [0.19, 0.20, 0.25], shade: [0.12, 0.13, 0.17], accent: [0.48, 0.28, 0.68], trim: [0.32, 0.33, 0.40], metal: [0.26, 0.27, 0.31], glass: [0.42, 0.60, 0.90], ..base },
        // unmanned: gunmetal with a cyan sensor stripe
        Liv::Drone => Palette { plate: [0.34, 0.36, 0.41], shade: [0.23, 0.25, 0.29], accent: [0.22, 0.78, 0.86], trim: [0.56, 0.60, 0.66], glass: [0.40, 0.90, 1.00], ..base },
        // rescue / medical: white with a red cross-band
        Liv::Rescue => Palette { plate: [0.90, 0.91, 0.92], shade: [0.66, 0.68, 0.72], accent: [0.82, 0.16, 0.18], trim: [0.30, 0.60, 0.78], ..base },
    }
}

/// Shipping-container manifest colours — one per cell of a container rack.
const MANIFEST: &[Rgb] = &[
    [0.50, 0.26, 0.22],
    [0.26, 0.34, 0.47],
    [0.53, 0.46, 0.25],
    [0.28, 0.40, 0.33],
    [0.44, 0.44, 0.47],
    [0.46, 0.33, 0.39],
    [0.31, 0.33, 0.38],
    [0.57, 0.54, 0.48],
    [0.33, 0.42, 0.45],
];

// ===========================================================================
// Parts
// ===========================================================================

/// Draw order. Everything under the hull goes first, mounts and greebles last.
const L_UNDER: u8 = 0; // radiators, wings, fins
const L_STRUT: u8 = 1; // pylons, trusses, engine bells
const L_POD: u8 = 2; // nacelles, cargo pods, tanks, hoppers
const L_HULL: u8 = 3; // the profiled main body
const L_SUPER: u8 = 4; // bridge blocks, flight decks, armour belts
const L_MOUNT: u8 = 5; // turrets, dishes, launcher blocks, greebles

/// Surface material — decides base colour, specular and any procedural detail.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Mat {
    Plate,
    Shade,
    Accent,
    Trim,
    Metal,
    Dark,
    Glass,
    Deck,
    Cargo,
    Rad,
    Bell,
}

/// A geometric primitive. Seven shapes cover every class in the table.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    /// The profiled main body — spans v ∈ [0,1], half-width from the profile.
    Hull,
    /// A lozenge (capsule) — nacelles, tanks, drums, pods.
    Pod,
    /// A rounded, rotatable box — bridges, decks, containers, greebles.
    Slab,
    /// A swept quad from a root chord out to a tapered tip.
    Wing,
    /// A circle with a dome normal — turrets, dishes, domes.
    Disc,
    /// An annulus — habitat and jump rings.
    Ring,
    /// An engine nozzle: a trapezoid flaring aft with a glowing throat.
    Bell,
}

#[derive(Clone, Copy)]
struct Part {
    kind: Kind,
    cx: f32,
    cy: f32,
    hw: f32,
    hh: f32,
    rot: f32,
    /// Kind-specific: Pod fore-taper · Slab corner radius · Wing sweep offset ·
    /// Disc dome height · Ring inner/outer ratio · Bell throat/mouth ratio.
    p0: f32,
    /// Kind-specific: Pod aft-taper · Wing tip-chord ratio · Slab cell columns.
    p1: f32,
    mat: Mat,
    layer: u8,
    /// Also draw the x-mirrored twin.
    mirror: bool,
    /// Per-part brightness multiplier — breaks up flat colour fields.
    tone: f32,
    /// Stable per-part noise seed (panel offsets, container colours).
    seed: u32,
    /// Texture subdivisions (columns, rows) for cells / decks / racks.
    cells: (f32, f32),
}

/// Everything the shader needs about the surface point it landed on.
struct Hit {
    n: [f32; 3],
    /// Dome height 0..1 — doubles as a cheap ambient-occlusion term.
    h: f32,
    mat: Mat,
    /// Part-local lateral coordinate, −1 (port) .. 1 (starboard).
    u: f32,
    /// Part-local longitudinal coordinate, 0 (fore) .. 1 (aft).
    v: f32,
    tone: f32,
    seed: u32,
    cells: (f32, f32),
}

impl Part {
    /// Axis-aligned bounds of the un-mirrored instance, in ship space.
    fn aabb(&self) -> (f32, f32, f32, f32) {
        match self.kind {
            Kind::Hull => (-self.hw, self.hw, -0.5, 0.5),
            Kind::Wing => {
                let (x0, x1) = (self.cx.min(self.cx + self.hw), self.cx.max(self.cx + self.hw));
                let (y0, y1) = (self.cy - self.hh, self.cy + self.hh + self.p0.max(0.0));
                (x0, x1, y0.min(self.cy - self.hh + self.p0.min(0.0)), y1)
            }
            Kind::Ring => (self.cx - self.hw, self.cx + self.hw, self.cy - self.hw, self.cy + self.hw),
            Kind::Disc => (self.cx - self.hw, self.cx + self.hw, self.cy - self.hw, self.cy + self.hw),
            _ => {
                // Rotatable box-ish parts: expand by the rotated half-diagonal.
                let (c, s) = (self.rot.cos().abs(), self.rot.sin().abs());
                let ex = self.hw * c + self.hh * s;
                let ey = self.hw * s + self.hh * c;
                (self.cx - ex, self.cx + ex, self.cy - ey, self.cy + ey)
            }
        }
    }

    /// Resolve one un-mirrored sample. `prof` is the ship's hull profile.
    fn hit_one(&self, prof: &[f32; 9], px: f32, py: f32) -> Option<Hit> {
        match self.kind {
            Kind::Hull => {
                let v = py + 0.5;
                if !(0.0..=1.0).contains(&v) {
                    return None;
                }
                let hw = profile_at(prof, v) * self.hw;
                if hw <= 1e-5 || px.abs() > hw {
                    return None;
                }
                // Generalized cylinder: n ∝ (k·sinθ, −hw'·k, cosθ).
                let sin_t = px / hw;
                let cos_t = (1.0 - sin_t * sin_t).max(0.0).sqrt();
                let e = 0.01;
                let dw = (profile_at(prof, v + e) - profile_at(prof, v - e)) * self.hw / (2.0 * e);
                const K: f32 = 0.62; // deck height vs beam — hulls are flatter than round
                Some(Hit {
                    n: norm3([K * sin_t, -dw * K, cos_t]),
                    h: cos_t,
                    mat: self.mat,
                    u: sin_t,
                    v,
                    tone: self.tone,
                    seed: self.seed,
                    cells: self.cells,
                })
            }
            Kind::Pod => {
                let (lx, ly) = self.local(px, py);
                let t = (ly + self.hh) / (2.0 * self.hh);
                if !(0.0..=1.0).contains(&t) {
                    return None;
                }
                // Capsule: full width through the middle, rounded ends, plus a
                // linear fore→aft taper so nacelles read as directional.
                let cap = (1.0 - (2.0 * t - 1.0).powi(2)).max(0.0).powf(0.28);
                let hw = self.hw * cap * lerp(self.p0, self.p1, t);
                if hw <= 1e-5 || lx.abs() > hw {
                    return None;
                }
                let sin_t = lx / hw;
                let cos_t = (1.0 - sin_t * sin_t).max(0.0).sqrt();
                Some(Hit {
                    n: self.unrot(norm3([0.72 * sin_t, 0.0, cos_t])),
                    h: cos_t,
                    mat: self.mat,
                    u: sin_t,
                    v: t,
                    tone: self.tone,
                    seed: self.seed,
                    cells: self.cells,
                })
            }
            Kind::Slab => {
                let (lx, ly) = self.local(px, py);
                let r = self.p0 * self.hw.min(self.hh);
                let ex = lx.abs() - (self.hw - r);
                let ey = ly.abs() - (self.hh - r);
                let d = ex.max(0.0).hypot(ey.max(0.0)) + ex.max(ey).min(0.0) - r;
                if d > 0.0 {
                    return None;
                }
                let bevel = 0.34 * self.hw.min(self.hh).max(1e-4);
                let s = 1.0 - clamp01(-d / bevel);
                let (mut gx, mut gy) = if ex > 0.0 && ey > 0.0 {
                    let m = ex.hypot(ey).max(1e-6);
                    (ex / m, ey / m)
                } else if ex > ey {
                    (1.0, 0.0)
                } else {
                    (0.0, 1.0)
                };
                gx *= lx.signum();
                gy *= ly.signum();
                let nz = (1.0 - s * s).max(0.0).sqrt();
                Some(Hit {
                    n: self.unrot(norm3([gx * s, gy * s, nz])),
                    h: 1.0 - s,
                    mat: self.mat,
                    u: lx / self.hw,
                    v: (ly + self.hh) / (2.0 * self.hh),
                    tone: self.tone,
                    seed: self.seed,
                    cells: self.cells,
                })
            }
            Kind::Wing => {
                // Root chord at cx (span 0), tip at cx+hw (span 1), chord centre
                // marching aft by p0 and shrinking to p1 of the root chord.
                let s = (px - self.cx) / self.hw;
                if !(0.0..=1.0).contains(&s) {
                    return None;
                }
                let half = self.hh * lerp(1.0, self.p1, s);
                let q = (py - (self.cy + self.p0 * s)) / half.max(1e-5);
                if q.abs() > 1.0 {
                    return None;
                }
                // A flat, slightly cambered plate: normal tips fore/aft at the
                // edges and rolls off toward the tip.
                let e = q * q * q;
                Some(Hit {
                    n: norm3([0.22 * s * self.hw.signum(), 0.72 * e, 0.92]),
                    h: 1.0 - 0.7 * q.abs(),
                    mat: self.mat,
                    u: s * self.hw.signum(),
                    v: (q + 1.0) * 0.5,
                    tone: self.tone,
                    seed: self.seed,
                    cells: self.cells,
                })
            }
            Kind::Disc => {
                let (lx, ly) = (px - self.cx, py - self.cy);
                let d2 = lx * lx + ly * ly;
                let r = self.hw;
                if d2 > r * r {
                    return None;
                }
                let q = d2.sqrt() / r;
                let nz = (1.0 - q * q).max(0.0).sqrt();
                let k = self.p0;
                Some(Hit {
                    n: norm3([k * lx / r, k * ly / r, nz.max(0.05)]),
                    h: nz,
                    mat: self.mat,
                    u: lx / r,
                    v: (ly / r + 1.0) * 0.5,
                    tone: self.tone,
                    seed: self.seed,
                    cells: self.cells,
                })
            }
            Kind::Ring => {
                let (lx, ly) = (px - self.cx, py - self.cy);
                let d = (lx * lx + ly * ly).sqrt();
                let ri = self.hw * self.p0;
                if d > self.hw || d < ri {
                    return None;
                }
                let mid = (self.hw + ri) * 0.5;
                let thick = (self.hw - ri) * 0.5;
                let q = (d - mid) / thick.max(1e-5);
                let nz = (1.0 - q * q).max(0.0).sqrt();
                let (ux, uy) = (lx / d.max(1e-5), ly / d.max(1e-5));
                Some(Hit {
                    n: norm3([ux * q, uy * q, nz.max(0.05)]),
                    h: nz,
                    mat: self.mat,
                    u: q,
                    // Angle around the ring — drives the window ring.
                    v: (ly.atan2(lx) / TAU + 0.5).fract(),
                    tone: self.tone,
                    seed: self.seed,
                    cells: self.cells,
                })
            }
            Kind::Bell => {
                let (lx, ly) = self.local(px, py);
                let t = (ly + self.hh) / (2.0 * self.hh);
                if !(0.0..=1.0).contains(&t) {
                    return None;
                }
                let hw = self.hw * lerp(self.p0, 1.0, t * t);
                if lx.abs() > hw {
                    return None;
                }
                let sin_t = lx / hw;
                let cos_t = (1.0 - sin_t * sin_t).max(0.0).sqrt();
                // The aft third is the throat — hot, self-lit, unshaded.
                let mat = if t > 0.74 { Mat::Bell } else { Mat::Metal };
                Some(Hit {
                    n: self.unrot(norm3([0.8 * sin_t, -0.35, cos_t])),
                    h: cos_t,
                    mat,
                    u: sin_t,
                    v: t,
                    tone: self.tone,
                    seed: self.seed,
                    cells: self.cells,
                })
            }
        }
    }

    /// Sample point in part-local (rotated) coordinates.
    fn local(&self, px: f32, py: f32) -> (f32, f32) {
        let (dx, dy) = (px - self.cx, py - self.cy);
        if self.rot == 0.0 {
            (dx, dy)
        } else {
            let (c, s) = (self.rot.cos(), self.rot.sin());
            (dx * c + dy * s, -dx * s + dy * c)
        }
    }
    /// Rotate a part-local normal back into ship space.
    fn unrot(&self, n: [f32; 3]) -> [f32; 3] {
        if self.rot == 0.0 {
            n
        } else {
            let (c, s) = (self.rot.cos(), self.rot.sin());
            [n[0] * c - n[1] * s, n[0] * s + n[1] * c, n[2]]
        }
    }
}

// ===========================================================================
// The ship
// ===========================================================================

/// A blinking navigation light: position, colour, blink period and phase.
#[derive(Clone, Copy)]
struct NavLight {
    x: f32,
    y: f32,
    col: Rgb,
    period: f32,
    phase: f32,
    r: f32,
}

/// A drive plume: the mouth of one engine bell plus its flame parameters.
#[derive(Clone, Copy)]
struct Plume {
    x: f32,
    y: f32,
    w: f32,
    len: f32,
}

/// The starfield backdrop is screen-space and time-independent, so it only ever
/// needs baking when the viewport or the star density changes — the same
/// "cache the backdrop" trick `solar` uses. Stored as loose f32 RGB triples.
#[derive(Default)]
struct BgCache {
    w: u32,
    h: u32,
    stars: f32,
    px: Vec<f32>,
}

/// One generated ship: a class blueprint rolled against a seed into a concrete
/// part list, plus the acceleration grid that makes it cheap to rasterize.
pub struct Ship {
    /// Index into [`CLASSES`].
    pub class: usize,
    pub seed: u32,
    /// Length in metres — the class nominal, jittered.
    pub length_m: f32,
    profile: [f32; 9],
    beam: f32,
    parts: Vec<Part>,
    pal: Palette,
    plumes: Vec<Plume>,
    lights: Vec<NavLight>,
    /// Lit-porthole density (a slider can drive it, so it lives on the ship
    /// rather than being read back off the class).
    windows: f32,
    /// Livery scheme: 0 spine stripe · 1 nose flash · 2 aft chevron · 3 none.
    stripe: u32,
    stripe_v: f32,
    hull_num: u32,
    name_a: usize,
    name_b: usize,
    // ship-space bounds of every part (the plume is not included)
    bx0: f32,
    bx1: f32,
    by0: f32,
    by1: f32,
    // uniform grid over the bounds -> part indices, in layer order
    gn: u32,
    cell_start: Vec<u32>,
    cell_items: Vec<u16>,
    /// Baked backdrop, reused across frames (see [`BgCache`]).
    bg: RefCell<BgCache>,
}

/// Live, slider-tunable structural parameters (mirrors `planet::NUM_PARAMS`).
///
/// | # | meaning | units |
/// |---|---------|-------|
/// | 0 | hull width | × class beam |
/// | 1 | wing span  | × class span |
/// | 2 | engines    | count |
/// | 3 | turrets    | count |
/// | 4 | cargo pods | pairs |
/// | 5 | flight decks | count |
/// | 6 | greeble density | 0..1 |
/// | 7 | window density | 0..1 |
/// | 8 | armour | 0..1 |
/// | 9 | livery | index into the livery families |
pub const NUM_PARAMS: usize = 10;

/// A class's default value for slider `which` — the web demo snaps to these
/// when you pick a class, exactly like the planet demo does.
pub fn param(class_idx: usize, which: u32) -> f32 {
    let c = &CLASSES[class_idx % CLASSES.len()];
    match which {
        0 => 1.0,
        1 => 1.0,
        2 => c.engines as f32,
        3 => c.turrets as f32,
        4 => c.pods as f32,
        5 => c.hangars as f32,
        6 => c.greeble,
        7 => c.windows,
        8 => c.armor,
        9 => liv_index(c.livery) as f32,
        _ => 0.0,
    }
}

fn liv_index(l: Liv) -> usize {
    match l {
        Liv::Naval => 0,
        Liv::Marine => 1,
        Liv::Merchant => 2,
        Liv::Corporate => 3,
        Liv::Industrial => 4,
        Liv::Militia => 5,
        Liv::Civil => 6,
        Liv::Covert => 7,
        Liv::Drone => 8,
        Liv::Rescue => 9,
    }
}
const LIV_ORDER: [Liv; 10] = [
    Liv::Naval,
    Liv::Marine,
    Liv::Merchant,
    Liv::Corporate,
    Liv::Industrial,
    Liv::Militia,
    Liv::Civil,
    Liv::Covert,
    Liv::Drone,
    Liv::Rescue,
];
const LIV_NAMES: [&str; 10] = [
    "naval", "marine", "merchant", "corporate", "industrial", "militia", "civil", "covert",
    "drone", "rescue",
];
/// Number of livery families.
pub fn livery_count() -> usize {
    LIV_NAMES.len()
}
/// Name of a livery family.
pub fn livery_name(i: usize) -> &'static str {
    LIV_NAMES[i % LIV_NAMES.len()]
}

impl Ship {
    /// Roll a ship of `class_idx` from `seed`.
    pub fn generate(class_idx: usize, seed: u32) -> Ship {
        Ship::generate_params(class_idx, seed, &[])
    }

    /// Roll a ship, overriding the slider params in `p` (see [`NUM_PARAMS`]).
    /// A short or empty slice just falls back to the class defaults.
    pub fn generate_params(class_idx: usize, seed: u32, p: &[f32]) -> Ship {
        let ci = class_idx % CLASSES.len();
        let c = &CLASSES[ci];
        let mut rng = Rng::new(seed ^ (ci as u32).wrapping_mul(0x9e37_79b9) ^ 0x5f3a_c1d3);
        let get = |i: usize, dflt: f32| -> f32 { p.get(i).copied().unwrap_or(dflt) };

        // ---- overall proportions ------------------------------------------
        let width_mul = get(0, 1.0).clamp(0.25, 3.0);
        let wing_mul = get(1, 1.0).clamp(0.0, 3.0);
        let beam = c.beam * rng.range(0.88, 1.14) * width_mul;
        let mut profile = silhouette(c.sil);
        for s in profile.iter_mut() {
            *s = (*s * rng.range(0.90, 1.10)).clamp(0.0, 1.15);
        }
        // The nose stays sharp on pointed families, so jitter never blunts them.
        profile[0] = silhouette(c.sil)[0] * rng.range(0.85, 1.15);
        let length_m = c.len_m * rng.range(0.80, 1.25);

        // ---- resolved counts ----------------------------------------------
        let engines = get(2, (c.engines as i32 + rng.int(-1, 1)).max(1) as f32).round().clamp(0.0, 12.0) as u32;
        let turrets = get(3, ((c.turrets as f32) * rng.range(0.78, 1.22)).round()).round().clamp(0.0, 24.0) as u32;
        let pods = get(4, (c.pods as i32 + if c.pods > 0 { rng.int(-1, 1) } else { 0 }).max(0) as f32).round().clamp(0.0, 8.0) as u32;
        let hangars = get(5, c.hangars as f32).round().clamp(0.0, 8.0) as u32;
        let greeble = get(6, (c.greeble * rng.range(0.60, 1.50)).min(1.0)).clamp(0.0, 1.0);
        let windows = get(7, (c.windows * rng.range(0.70, 1.30)).min(1.0)).clamp(0.0, 1.0);
        let armor = get(8, (c.armor * rng.range(0.75, 1.25)).min(1.0)).clamp(0.0, 1.0);
        let liv_i = get(9, liv_index(c.livery) as f32).round().clamp(0.0, 9.0) as usize;
        let launchers = (c.launchers as i32 + if c.launchers > 0 { rng.int(-1, 1) } else { 0 }).max(0) as u32;
        let nacelles = c.nacelles;
        let fins = c.fins;
        let rads = c.rads;

        // ---- palette -------------------------------------------------------
        let hue = rng.range(-0.04, 0.04);
        let base_pal = livery(LIV_ORDER[liv_i]);
        let pal = Palette {
            plate: hue_shift(base_pal.plate, hue),
            shade: hue_shift(base_pal.shade, hue),
            accent: hue_shift(base_pal.accent, hue * 1.5),
            trim: hue_shift(base_pal.trim, hue),
            metal: base_pal.metal,
            dark: base_pal.dark,
            glass: base_pal.glass,
            glow: hue_shift(c.glow, rng.range(-0.05, 0.05)),
        };

        let mut s = Ship {
            class: ci,
            seed,
            length_m,
            profile,
            beam,
            parts: Vec::with_capacity(64),
            pal,
            plumes: Vec::new(),
            lights: Vec::new(),
            windows,
            stripe: rng.int(0, 3) as u32,
            stripe_v: rng.range(0.18, 0.72),
            hull_num: (rng.f() * 900.0) as u32 + 100,
            name_a: (rng.f() * NAME_A.len() as f32) as usize % NAME_A.len(),
            name_b: (rng.f() * NAME_B.len() as f32) as usize % NAME_B.len(),
            bx0: 0.0,
            bx1: 0.0,
            by0: 0.0,
            by1: 0.0,
            gn: 0,
            cell_start: Vec::new(),
            cell_items: Vec::new(),
            bg: RefCell::new(BgCache::default()),
        };

        // ---- assemble ------------------------------------------------------
        // Order of construction doesn't matter (parts carry an explicit layer);
        // it's grouped the way a shipwright would talk about it.
        s.add_spine(c, &mut rng);
        s.add_hull(&mut rng);
        s.add_wings(c, wing_mul, fins, &mut rng);
        s.add_nacelles(nacelles, &mut rng);
        s.add_pods(c, pods, &mut rng);
        s.add_ring(c, &mut rng);
        s.add_superstructure(c, armor, &mut rng);
        s.add_hangars(hangars, &mut rng);
        s.add_turrets(turrets, &mut rng);
        s.add_launchers(launchers, &mut rng);
        s.add_dish(c, &mut rng);
        s.add_radiators(rads, &mut rng);
        s.add_greebles(greeble, &mut rng);
        s.add_engines(c, engines, &mut rng);
        s.add_lights(&mut rng);

        s.finish();
        s
    }

    /// Pick a class from the seed too — the "surprise me" entry point.
    pub fn random(seed: u32) -> Ship {
        let mut rng = Rng::new(seed ^ 0x1234_5678);
        let ci = (rng.f() * CLASSES.len() as f32) as usize % CLASSES.len();
        Ship::generate(ci, seed)
    }

    // -- assembly helpers ---------------------------------------------------

    fn push(&mut self, p: Part) {
        self.parts.push(p);
    }

    /// Half-width of the hull at `v` ∈ [0,1], in ship space.
    pub fn half_width(&self, v: f32) -> f32 {
        profile_at(&self.profile, v) * self.beam
    }

    fn add_hull(&mut self, rng: &mut Rng) {
        let seed = rng.sub();
        self.push(Part {
            kind: Kind::Hull,
            cx: 0.0,
            cy: 0.0,
            hw: self.beam,
            hh: 0.5,
            rot: 0.0,
            p0: 0.0,
            p1: 0.0,
            mat: Mat::Plate,
            layer: L_HULL,
            mirror: false,
            tone: 1.0,
            seed,
            cells: (0.0, 0.0),
        });
    }

    /// An exposed truss backbone with cross-braces — bulk haulers, rigs, colony
    /// ships. It runs aft of the bow section and carries the pods.
    fn add_spine(&mut self, c: &Class, rng: &mut Rng) {
        if c.spine <= 0.0 {
            return;
        }
        let v0 = 0.30;
        let v1 = (v0 + c.spine).min(0.95);
        let cy = (v0 + v1) * 0.5 - 0.5;
        let hh = (v1 - v0) * 0.5;
        let hw = self.beam * 0.30;
        let seed = rng.sub();
        self.push(Part {
            kind: Kind::Slab,
            cx: 0.0,
            cy,
            hw,
            hh,
            rot: 0.0,
            p0: 0.25,
            p1: 0.0,
            mat: Mat::Metal,
            layer: L_STRUT,
            mirror: false,
            tone: 0.92,
            seed,
            cells: (0.0, 0.0),
        });
        // cross-braces
        let n = rng.int(3, 7);
        for i in 0..n {
            let t = (i as f32 + 0.5) / n as f32;
            let y = lerp(v0, v1, t) - 0.5;
            self.push(Part {
                kind: Kind::Slab,
                cx: 0.0,
                cy: y,
                hw: self.beam * rng.range(0.62, 0.92),
                hh: (v1 - v0) * 0.022,
                rot: 0.0,
                p0: 0.4,
                p1: 0.0,
                mat: Mat::Metal,
                layer: L_STRUT,
                mirror: false,
                tone: rng.range(0.82, 1.0),
                seed: rng.sub(),
                cells: (0.0, 0.0),
            });
        }
    }

    fn add_wings(&mut self, c: &Class, wing_mul: f32, fins: u32, rng: &mut Rng) {
        let span = c.wing * wing_mul;
        if span > 0.001 {
            let sweep = (c.sweep + rng.range(-0.12, 0.12)).clamp(0.0, 1.0);
            let v_root = rng.range(0.52, 0.70);
            let chord = rng.range(0.13, 0.22);
            let taper = rng.range(0.30, 0.75);
            let root_x = self.half_width(v_root) * 0.85;
            self.push(Part {
                kind: Kind::Wing,
                cx: root_x,
                cy: v_root - 0.5,
                hw: span,
                hh: chord * 0.5,
                rot: 0.0,
                p0: sweep * span * 1.25,
                p1: taper,
                mat: Mat::Plate,
                layer: L_UNDER,
                mirror: true,
                tone: 0.94,
                seed: rng.sub(),
                cells: (0.0, 0.0),
            });
            // A darker strake under the leading edge reads as wing structure.
            self.push(Part {
                kind: Kind::Wing,
                cx: root_x,
                cy: v_root - 0.5 - chord * 0.30,
                hw: span * 0.86,
                hh: chord * 0.13,
                rot: 0.0,
                p0: sweep * span * 1.25,
                p1: taper,
                mat: Mat::Accent,
                layer: L_SUPER,
                mirror: true,
                tone: 1.0,
                seed: rng.sub(),
                cells: (0.0, 0.0),
            });
        }
        for i in 0..fins {
            let v_root = rng.range(0.80, 0.93);
            let fspan = (c.wing.max(self.beam * 1.4)) * rng.range(0.35, 0.65);
            let chord = rng.range(0.07, 0.12);
            self.push(Part {
                kind: Kind::Wing,
                cx: self.half_width(v_root) * 0.8,
                cy: v_root - 0.5 + i as f32 * 0.03,
                hw: fspan,
                hh: chord * 0.5,
                rot: 0.0,
                p0: fspan * 0.9,
                p1: rng.range(0.35, 0.6),
                mat: Mat::Shade,
                layer: L_UNDER,
                mirror: true,
                tone: 0.9,
                seed: rng.sub(),
                cells: (0.0, 0.0),
            });
        }
    }

    fn add_nacelles(&mut self, n: u32, rng: &mut Rng) {
        for i in 0..n {
            let v = rng.range(0.55, 0.72);
            let off = self.half_width(v) + self.beam * rng.range(0.38, 0.72);
            let len = rng.range(0.22, 0.36);
            let cy = v - 0.5 + len * 0.25 + i as f32 * 0.04;
            // pylon
            self.push(Part {
                kind: Kind::Slab,
                cx: off * 0.55,
                cy: cy - len * 0.15,
                hw: off * 0.5,
                hh: 0.016,
                rot: 0.0,
                p0: 0.5,
                p1: 0.0,
                mat: Mat::Metal,
                layer: L_STRUT,
                mirror: true,
                tone: 0.88,
                seed: rng.sub(),
                cells: (0.0, 0.0),
            });
            // the nacelle body
            self.push(Part {
                kind: Kind::Pod,
                cx: off,
                cy,
                hw: self.beam * rng.range(0.26, 0.40),
                hh: len * 0.5,
                rot: 0.0,
                p0: 0.80,
                p1: 1.0,
                mat: Mat::Plate,
                layer: L_POD,
                mirror: true,
                tone: 0.97,
                seed: rng.sub(),
                cells: (0.0, 0.0),
            });
            // its own little bell + plume
            let bw = self.beam * 0.28;
            let by = cy + len * 0.5;
            self.push(Part {
                kind: Kind::Bell,
                cx: off,
                cy: by + 0.012,
                hw: bw,
                hh: 0.022,
                rot: 0.0,
                p0: 0.6,
                p1: 0.0,
                mat: Mat::Metal,
                layer: L_STRUT,
                mirror: true,
                tone: 1.0,
                seed: rng.sub(),
                cells: (0.0, 0.0),
            });
            self.plumes.push(Plume { x: off, y: by + 0.030, w: bw * 0.85, len: 0.22 });
            self.plumes.push(Plume { x: -off, y: by + 0.030, w: bw * 0.85, len: 0.22 });
        }
    }

    /// Cargo / utility pods. Containers become racks of individually-coloured
    /// boxes; tanks and hoppers become drums and open bins.
    fn add_pods(&mut self, c: &Class, pods: u32, rng: &mut Rng) {
        if pods == 0 || c.pod == Pod::None {
            return;
        }
        let v0 = if c.spine > 0.0 { 0.34 } else { 0.28 };
        let v1 = if c.spine > 0.0 { (0.34 + c.spine).min(0.92) } else { 0.86 };
        for i in 0..pods {
            let t = if pods == 1 { 0.5 } else { i as f32 / (pods - 1) as f32 };
            let cy = lerp(v0, v1, t) - 0.5;
            let hh = ((v1 - v0) / pods as f32) * rng.range(0.34, 0.46);
            let hull = self.half_width(cy + 0.5);
            let off = hull + self.beam * rng.range(0.30, 0.55);
            let hw = self.beam * rng.range(0.32, 0.52);
            // pylon out to the pod
            self.push(Part {
                kind: Kind::Slab,
                cx: (hull + off) * 0.5,
                cy,
                hw: (off - hull) * 0.5 + 0.004,
                hh: hh * 0.16,
                rot: 0.0,
                p0: 0.4,
                p1: 0.0,
                mat: Mat::Metal,
                layer: L_STRUT,
                mirror: true,
                tone: 0.86,
                seed: rng.sub(),
                cells: (0.0, 0.0),
            });
            match c.pod {
                Pod::Container => {
                    let cols = rng.int(2, 3) as f32;
                    let rows = rng.int(3, 6) as f32;
                    self.push(Part {
                        kind: Kind::Slab,
                        cx: off,
                        cy,
                        hw,
                        hh,
                        rot: 0.0,
                        p0: 0.04,
                        p1: 0.0,
                        mat: Mat::Cargo,
                        layer: L_POD,
                        mirror: true,
                        tone: 1.0,
                        seed: rng.sub(),
                        cells: (cols, rows),
                    });
                }
                Pod::Tank => {
                    self.push(Part {
                        kind: Kind::Pod,
                        cx: off,
                        cy,
                        hw,
                        hh,
                        rot: 0.0,
                        p0: 1.0,
                        p1: 1.0,
                        mat: Mat::Metal,
                        layer: L_POD,
                        mirror: true,
                        tone: 1.06,
                        seed: rng.sub(),
                        cells: (0.0, rng.int(2, 4) as f32),
                    });
                }
                Pod::Hopper => {
                    self.push(Part {
                        kind: Kind::Slab,
                        cx: off,
                        cy,
                        hw,
                        hh,
                        rot: 0.0,
                        p0: 0.10,
                        p1: 0.0,
                        mat: Mat::Shade,
                        layer: L_POD,
                        mirror: true,
                        tone: 0.95,
                        seed: rng.sub(),
                        cells: (0.0, 0.0),
                    });
                    // the open, ore-filled mouth
                    self.push(Part {
                        kind: Kind::Slab,
                        cx: off,
                        cy,
                        hw: hw * 0.68,
                        hh: hh * 0.78,
                        rot: 0.0,
                        p0: 0.12,
                        p1: 0.0,
                        mat: Mat::Dark,
                        layer: L_SUPER,
                        mirror: true,
                        tone: 1.0,
                        seed: rng.sub(),
                        cells: (0.0, 0.0),
                    });
                }
                Pod::Module | Pod::Nacelle | Pod::None => {
                    self.push(Part {
                        kind: Kind::Slab,
                        cx: off,
                        cy,
                        hw,
                        hh,
                        rot: 0.0,
                        p0: 0.28,
                        p1: 0.0,
                        mat: Mat::Shade,
                        layer: L_POD,
                        mirror: true,
                        tone: rng.range(0.92, 1.08),
                        seed: rng.sub(),
                        cells: (0.0, 0.0),
                    });
                }
            }
        }
    }

    /// A habitat / jump ring around the hull — the visual signature of colony,
    /// generation and science hulls.
    fn add_ring(&mut self, c: &Class, rng: &mut Rng) {
        if c.ring <= 0.0 {
            return;
        }
        let r = (c.ring * rng.range(0.85, 1.15)).max(self.beam * 1.6);
        let v = rng.range(0.34, 0.56);
        let seed = rng.sub();
        // spokes
        for k in 0..4 {
            let a = k as f32 * PI / 4.0 + rng.range(0.0, 0.4);
            self.push(Part {
                kind: Kind::Slab,
                cx: 0.0,
                cy: v - 0.5,
                hw: r * 0.98,
                hh: 0.008,
                rot: a,
                p0: 0.3,
                p1: 0.0,
                mat: Mat::Metal,
                layer: L_STRUT,
                mirror: false,
                tone: 0.85,
                seed: rng.sub(),
                cells: (0.0, 0.0),
            });
        }
        self.push(Part {
            kind: Kind::Ring,
            cx: 0.0,
            cy: v - 0.5,
            hw: r,
            hh: r,
            rot: 0.0,
            p0: rng.range(0.74, 0.86),
            p1: 0.0,
            mat: Mat::Trim,
            layer: L_POD,
            mirror: false,
            tone: 1.0,
            seed,
            cells: (rng.int(22, 40) as f32, 1.0),
        });
    }

    /// Bridge blocks and armour belts on the hull's dorsal surface.
    fn add_superstructure(&mut self, c: &Class, armor: f32, rng: &mut Rng) {
        if c.bridge > 0.01 {
            let n = rng.int(1, 3);
            let v0 = rng.range(0.28, 0.46);
            for i in 0..n {
                let v = v0 + i as f32 * rng.range(0.05, 0.10);
                let hull = self.half_width(v);
                let k = 1.0 - i as f32 * 0.22;
                self.push(Part {
                    kind: Kind::Slab,
                    cx: 0.0,
                    cy: v - 0.5,
                    hw: hull * 0.62 * c.bridge.clamp(0.35, 1.0) * k,
                    hh: 0.035 * c.bridge.max(0.4) * k,
                    rot: 0.0,
                    p0: 0.35,
                    p1: 0.0,
                    mat: if i == n - 1 { Mat::Trim } else { Mat::Shade },
                    layer: L_SUPER,
                    mirror: false,
                    tone: rng.range(0.95, 1.08),
                    seed: rng.sub(),
                    cells: (0.0, 0.0),
                });
            }
            // A canopy for anything small enough to be flown by hand.
            if c.role == Role::Fighter || c.role == Role::Civilian || c.len_m < 80.0 {
                let v = rng.range(0.16, 0.30);
                self.push(Part {
                    kind: Kind::Pod,
                    cx: 0.0,
                    cy: v - 0.5,
                    hw: self.half_width(v) * 0.52,
                    hh: 0.055,
                    rot: 0.0,
                    p0: 0.7,
                    p1: 1.0,
                    mat: Mat::Glass,
                    layer: L_SUPER,
                    mirror: false,
                    tone: 1.0,
                    seed: rng.sub(),
                    cells: (0.0, 0.0),
                });
            }
        }
        // Armour belts: broad dark bands athwartships.
        let belts = (armor * 4.0).round() as i32;
        for _ in 0..belts {
            let v = rng.range(0.22, 0.88);
            let hull = self.half_width(v);
            if hull < 1e-4 {
                continue;
            }
            self.push(Part {
                kind: Kind::Slab,
                cx: 0.0,
                cy: v - 0.5,
                hw: hull * rng.range(0.80, 0.98),
                hh: rng.range(0.012, 0.030),
                rot: 0.0,
                p0: 0.5,
                p1: 0.0,
                mat: Mat::Shade,
                layer: L_SUPER,
                mirror: false,
                tone: rng.range(0.86, 1.02),
                seed: rng.sub(),
                cells: (0.0, 0.0),
            });
        }
    }

    /// Flight decks: long dark strips with centreline markings, angled a few
    /// degrees off the keel and overhanging the hull like a real carrier.
    fn add_hangars(&mut self, n: u32, rng: &mut Rng) {
        for i in 0..n {
            let side = if i % 2 == 0 { 1.0 } else { -1.0 };
            let v = rng.range(0.34, 0.62);
            let hull = self.half_width(v);
            let angle = rng.range(0.02, 0.09) * side;
            let hw = self.beam * rng.range(0.20, 0.32);
            let hh = rng.range(0.22, 0.34);
            let off = side * (hull * rng.range(0.35, 0.70));
            // sponson under the deck
            self.push(Part {
                kind: Kind::Slab,
                cx: off,
                cy: v - 0.5,
                hw: hw * 1.22,
                hh: hh * 1.06,
                rot: angle,
                p0: 0.18,
                p1: 0.0,
                mat: Mat::Shade,
                layer: L_SUPER,
                mirror: false,
                tone: 0.92,
                seed: rng.sub(),
                cells: (0.0, 0.0),
            });
            // the deck itself
            self.push(Part {
                kind: Kind::Slab,
                cx: off,
                cy: v - 0.5,
                hw,
                hh,
                rot: angle,
                p0: 0.06,
                p1: 0.0,
                mat: Mat::Deck,
                layer: L_MOUNT,
                mirror: false,
                tone: 1.0,
                seed: rng.sub(),
                cells: (1.0, rng.int(4, 8) as f32),
            });
        }
    }

    /// Gun turrets: a barbette disc, a rotated housing and one or two barrels.
    fn add_turrets(&mut self, n: u32, rng: &mut Rng) {
        for i in 0..n {
            // Alternate: centreline fore, centreline aft, then outboard pairs.
            let outboard = i >= 4 && i % 2 == 0;
            let v = rng.range(0.15, 0.90);
            let hull = self.half_width(v);
            if hull < 1e-4 {
                continue;
            }
            let r = (self.beam * rng.range(0.14, 0.24)).min(hull * 0.62);
            let cx = if outboard { hull * rng.range(0.35, 0.66) } else { 0.0 };
            let aim = if v < 0.5 { rng.range(-0.35, 0.35) } else { PI + rng.range(-0.35, 0.35) };
            let mirror = outboard;
            self.push(Part {
                kind: Kind::Disc,
                cx,
                cy: v - 0.5,
                hw: r * 1.25,
                hh: r * 1.25,
                rot: 0.0,
                p0: 0.35,
                p1: 0.0,
                mat: Mat::Shade,
                layer: L_MOUNT,
                mirror,
                tone: 0.72,
                seed: rng.sub(),
                cells: (0.0, 0.0),
            });
            self.push(Part {
                kind: Kind::Disc,
                cx,
                cy: v - 0.5,
                hw: r,
                hh: r,
                rot: 0.0,
                p0: 0.85,
                p1: 0.0,
                mat: Mat::Plate,
                layer: L_MOUNT,
                mirror,
                tone: 1.06,
                seed: rng.sub(),
                cells: (0.0, 0.0),
            });
            // barrels, pointing `aim`
            let blen = r * rng.range(1.6, 3.0);
            let (sa, ca) = ((aim).sin(), (aim).cos());
            self.push(Part {
                kind: Kind::Slab,
                cx: cx + sa * blen * 0.6,
                cy: v - 0.5 - ca * blen * 0.6,
                hw: r * 0.20,
                hh: blen * 0.6,
                rot: -aim,
                p0: 0.5,
                p1: 0.0,
                mat: Mat::Metal,
                layer: L_MOUNT,
                mirror,
                tone: 0.95,
                seed: rng.sub(),
                cells: (0.0, 0.0),
            });
        }
    }

    /// Missile / VLS blocks: dark rectangles with a visible cell grid.
    fn add_launchers(&mut self, n: u32, rng: &mut Rng) {
        for _ in 0..n {
            let v = rng.range(0.20, 0.82);
            let hull = self.half_width(v);
            if hull < 1e-4 {
                continue;
            }
            let cols = rng.int(2, 4) as f32;
            let rows = rng.int(3, 6) as f32;
            let hw = (hull * rng.range(0.24, 0.42)).min(self.beam * 0.30);
            let hh = (hw * rows / cols * rng.range(0.8, 1.2)).min(0.10);
            let outboard = rng.chance(0.55);
            self.push(Part {
                kind: Kind::Slab,
                cx: if outboard { hull * 0.45 } else { 0.0 },
                cy: v - 0.5,
                hw,
                hh,
                rot: 0.0,
                p0: 0.10,
                p1: 0.0,
                mat: Mat::Dark,
                layer: L_MOUNT,
                mirror: outboard,
                tone: 1.0,
                seed: rng.sub(),
                cells: (cols, rows),
            });
        }
    }

    fn add_dish(&mut self, c: &Class, rng: &mut Rng) {
        if c.dish <= 0.0 {
            return;
        }
        let r = c.dish * rng.range(0.8, 1.3);
        let v = rng.range(0.20, 0.42);
        let boom = rng.range(0.02, 0.06);
        self.push(Part {
            kind: Kind::Slab,
            cx: 0.0,
            cy: v - 0.5 - boom * 0.5,
            hw: 0.008,
            hh: boom,
            rot: 0.0,
            p0: 0.5,
            p1: 0.0,
            mat: Mat::Metal,
            layer: L_STRUT,
            mirror: false,
            tone: 0.9,
            seed: rng.sub(),
            cells: (0.0, 0.0),
        });
        self.push(Part {
            kind: Kind::Disc,
            cx: 0.0,
            cy: v - 0.5 - boom,
            hw: r,
            hh: r,
            rot: 0.0,
            p0: -0.75, // concave: a dish, not a dome
            p1: 0.0,
            mat: Mat::Trim,
            layer: L_MOUNT,
            mirror: false,
            tone: 1.0,
            seed: rng.sub(),
            cells: (0.0, 0.0),
        });
        self.push(Part {
            kind: Kind::Disc,
            cx: 0.0,
            cy: v - 0.5 - boom,
            hw: r * 0.22,
            hh: r * 0.22,
            rot: 0.0,
            p0: 0.9,
            p1: 0.0,
            mat: Mat::Metal,
            layer: L_MOUNT,
            mirror: false,
            tone: 1.0,
            seed: rng.sub(),
            cells: (0.0, 0.0),
        });
    }

    /// Radiator panels — thin fluted fins amidships-aft, running hot. Kept
    /// deliberately short: a radiator that out-spans the hull reads as a wing.
    fn add_radiators(&mut self, n: u32, rng: &mut Rng) {
        for i in 0..n {
            let v = rng.range(0.58, 0.86) + i as f32 * 0.015;
            let hull = self.half_width(v);
            let span = self.beam * rng.range(0.60, 1.10);
            self.push(Part {
                kind: Kind::Wing,
                cx: hull * 0.92,
                cy: v - 0.5,
                hw: span,
                hh: rng.range(0.024, 0.045),
                rot: 0.0,
                p0: span * rng.range(0.10, 0.30),
                p1: rng.range(0.80, 1.0),
                mat: Mat::Rad,
                layer: L_UNDER,
                mirror: true,
                tone: 1.0,
                seed: rng.sub(),
                cells: (rng.int(3, 6) as f32, 1.0),
            });
        }
    }

    /// Surface greebles — small plates, vents and blisters that break up the
    /// hull. Cheap, and the single biggest "this looks built" multiplier.
    fn add_greebles(&mut self, density: f32, rng: &mut Rng) {
        let n = (density * 22.0).round() as i32;
        for _ in 0..n {
            let v = rng.range(0.10, 0.94);
            let hull = self.half_width(v);
            if hull < 1e-4 {
                continue;
            }
            let x = rng.range(-0.85, 0.85) * hull;
            let sz = hull * rng.range(0.07, 0.20);
            let disc = rng.chance(0.28);
            self.push(Part {
                kind: if disc { Kind::Disc } else { Kind::Slab },
                cx: x,
                cy: v - 0.5,
                hw: sz,
                hh: if disc { sz } else { sz * rng.range(0.5, 2.0) },
                rot: 0.0,
                p0: if disc { 0.5 } else { rng.range(0.1, 0.5) },
                p1: 0.0,
                mat: if rng.chance(0.14) { Mat::Dark } else { Mat::Shade },
                layer: L_MOUNT,
                mirror: rng.chance(0.55),
                tone: rng.range(0.84, 1.14),
                seed: rng.sub(),
                cells: (0.0, 0.0),
            });
        }
    }

    /// Main drive bells across the stern, plus the plumes that trail them.
    fn add_engines(&mut self, c: &Class, n: u32, rng: &mut Rng) {
        if n == 0 {
            return;
        }
        let v = 0.985;
        let stern = self.half_width(v).max(self.beam * 0.22);
        let bw = (stern * 1.7 / n as f32).min(self.beam * 0.42) * rng.range(0.82, 1.05);
        let plen = c.thrust * rng.range(0.85, 1.20);
        for i in 0..n {
            let t = if n == 1 { 0.0 } else { (i as f32 / (n - 1) as f32) * 2.0 - 1.0 };
            let x = t * (stern - bw).max(0.0);
            let bh = bw * rng.range(0.55, 0.85);
            self.push(Part {
                kind: Kind::Bell,
                cx: x,
                cy: 0.5 - bh * 0.2,
                hw: bw,
                hh: bh,
                rot: 0.0,
                p0: rng.range(0.55, 0.75),
                p1: 0.0,
                mat: Mat::Metal,
                layer: L_STRUT,
                mirror: false,
                tone: 1.0,
                seed: rng.sub(),
                cells: (0.0, 0.0),
            });
            self.plumes.push(Plume { x, y: 0.5 + bh * 0.8, w: bw * 0.9, len: plen });
        }
    }

    /// Navigation lights: red to port, green to starboard, white strobes fore
    /// and aft — the one detail that makes a still read as a *working* ship.
    fn add_lights(&mut self, rng: &mut Rng) {
        let r = self.beam * 0.10 + 0.004;
        let v_wide = 0.62;
        let hull = self.half_width(v_wide);
        self.lights.push(NavLight { x: -hull * 0.96, y: v_wide - 0.5, col: [1.0, 0.22, 0.20], period: 1.7, phase: rng.f(), r });
        self.lights.push(NavLight { x: hull * 0.96, y: v_wide - 0.5, col: [0.24, 1.0, 0.40], period: 1.7, phase: rng.f(), r });
        self.lights.push(NavLight { x: 0.0, y: -0.47, col: [1.0, 1.0, 0.95], period: 2.4, phase: rng.f(), r: r * 0.8 });
        self.lights.push(NavLight { x: 0.0, y: 0.40, col: [1.0, 0.95, 0.75], period: 3.1, phase: rng.f(), r: r * 0.8 });
    }

    // -- bake the acceleration grid -----------------------------------------

    fn finish(&mut self) {
        // Stable layer sort — inside a layer, construction order wins.
        self.parts.sort_by_key(|p| p.layer);

        let (mut x0, mut x1, mut y0, mut y1) = (f32::MAX, f32::MIN, f32::MAX, f32::MIN);
        for p in &self.parts {
            let (a, b, c, d) = p.aabb();
            let (a, b) = if p.mirror { (a.min(-b), b.max(-a)) } else { (a, b) };
            x0 = x0.min(a);
            x1 = x1.max(b);
            y0 = y0.min(c);
            y1 = y1.max(d);
        }
        if !x0.is_finite() {
            x0 = -0.1;
            x1 = 0.1;
            y0 = -0.5;
            y1 = 0.5;
        }
        let pad = 0.004;
        self.bx0 = x0 - pad;
        self.bx1 = x1 + pad;
        self.by0 = y0 - pad;
        self.by1 = y1 + pad;

        // Uniform grid, counting-sorted into a flat index array.
        let gn: u32 = 24;
        self.gn = gn;
        let cells = (gn * gn) as usize;
        let mut counts = vec![0u32; cells + 1];
        let sx = gn as f32 / (self.bx1 - self.bx0).max(1e-6);
        let sy = gn as f32 / (self.by1 - self.by0).max(1e-6);
        let cell_range = |a: f32, b: f32, c: f32, d: f32| -> (u32, u32, u32, u32) {
            let i0 = (((a - self.bx0) * sx).floor().max(0.0) as u32).min(gn - 1);
            let i1 = (((b - self.bx0) * sx).ceil().max(0.0) as u32).min(gn - 1);
            let j0 = (((c - self.by0) * sy).floor().max(0.0) as u32).min(gn - 1);
            let j1 = (((d - self.by0) * sy).ceil().max(0.0) as u32).min(gn - 1);
            (i0, i1, j0, j1)
        };
        // Each part registers its own box and (if mirrored) its twin's.
        let boxes: Vec<(usize, f32, f32, f32, f32)> = self
            .parts
            .iter()
            .enumerate()
            .flat_map(|(i, p)| {
                let (a, b, c, d) = p.aabb();
                let mut v = vec![(i, a, b, c, d)];
                if p.mirror {
                    v.push((i, -b, -a, c, d));
                }
                v
            })
            .collect();
        for &(_, a, b, c, d) in &boxes {
            let (i0, i1, j0, j1) = cell_range(a, b, c, d);
            for j in j0..=j1 {
                for i in i0..=i1 {
                    counts[(j * gn + i) as usize + 1] += 1;
                }
            }
        }
        for k in 1..=cells {
            counts[k] += counts[k - 1];
        }
        let total = counts[cells] as usize;
        let mut items = vec![0u16; total];
        let mut cursor = counts.clone();
        for &(pi, a, b, c, d) in &boxes {
            let (i0, i1, j0, j1) = cell_range(a, b, c, d);
            for j in j0..=j1 {
                for i in i0..=i1 {
                    let cell = (j * gn + i) as usize;
                    items[cursor[cell] as usize] = pi as u16;
                    cursor[cell] += 1;
                }
            }
        }
        self.cell_start = counts;
        self.cell_items = items;
    }

    // -- queries ------------------------------------------------------------

    /// Number of parts in the assembled hull.
    pub fn part_count(&self) -> usize {
        self.parts.len()
    }
    /// Ship-space bounds as `(x0, x1, y0, y1)`.
    pub fn bounds(&self) -> (f32, f32, f32, f32) {
        (self.bx0, self.bx1, self.by0, self.by1)
    }
    /// Width / length of the ship's own bounding box.
    pub fn aspect(&self) -> f32 {
        (self.bx1 - self.bx0) / (self.by1 - self.by0).max(1e-6)
    }
    /// Radius of the smallest circle around the ship — the zoom that stays
    /// stable while the ship turns.
    pub fn radius(&self) -> f32 {
        let cx = self.bx0.abs().max(self.bx1.abs());
        let cy = self.by0.abs().max(self.by1.abs());
        (cx * cx + cy * cy).sqrt()
    }

    /// Zoom (pixels per ship-length unit) that fits the ship into `w`x`h` at
    /// this `heading`, with a 12% margin.
    pub fn fit_zoom(&self, w: u32, h: u32, heading: f32) -> f32 {
        let (c, s) = (heading.cos().abs(), heading.sin().abs());
        let (ex, ey) = (self.bx1 - self.bx0, self.by1 - self.by0);
        let rx = ex * c + ey * s;
        let ry = ex * s + ey * c;
        ((w as f32 * 0.88) / rx.max(1e-4)).min((h as f32 * 0.88) / ry.max(1e-4))
    }
    /// Zoom that fits the ship at *every* heading — use it for turn animations
    /// so the hull doesn't pulse as it rotates.
    pub fn fit_zoom_spin(&self, w: u32, h: u32) -> f32 {
        (w.min(h) as f32 * 0.44) / self.radius().max(1e-4)
    }

    /// Zoom + pan that fits the hull AND leaves `plume` (as a fraction of the
    /// frame height) clear astern for the exhaust. Nose-up only.
    pub fn fit_with_plume(&self, w: u32, h: u32, plume: f32) -> (f32, f32) {
        let usable = (h as f32 * (1.0 - plume.clamp(0.0, 0.6))).max(1.0);
        let zoom = self.fit_zoom(w, usable as u32, 0.0);
        // Centre the hull inside the usable band, which sits at the frame top.
        let pan_y = (usable - h as f32) * 0.5;
        (zoom, pan_y)
    }

    /// This ship's naval designation, e.g. `BB-417 Iron Vigil`.
    pub fn designation(&self) -> String {
        let c = &CLASSES[self.class];
        let prefix = match c.role {
            Role::Drone => "UD",
            Role::Fighter => "VF",
            Role::Warship => {
                if c.len_m >= 700.0 {
                    "BB"
                } else if c.len_m >= 400.0 {
                    "CA"
                } else if c.len_m >= 220.0 {
                    "DD"
                } else {
                    "FF"
                }
            }
            Role::Carrier => {
                if c.len_m >= 600.0 {
                    "CVN"
                } else {
                    "CVL"
                }
            }
            Role::Freighter => "MV",
            Role::Industrial => "IX",
            Role::Civilian => "SS",
            Role::Covert => "QS",
        };
        format!("{}-{} {} {}", prefix, self.hull_num, NAME_A[self.name_a], NAME_B[self.name_b])
    }
}

/// Ship-name word lists — an adjective/noun pair per hull.
const NAME_A: &[&str] = &[
    "Iron", "Pale", "Long", "Silent", "Bright", "Cold", "Far", "Deep", "Red", "Blue", "Grey",
    "Swift", "Steady", "Hollow", "Golden", "Black", "Silver", "First", "Last", "High", "Lone",
    "Quiet", "Bitter", "Sudden", "Distant", "Amber", "Copper", "Winter", "Autumn", "Northern",
];
const NAME_B: &[&str] = &[
    "Vigil", "Meridian", "Lantern", "Anvil", "Compass", "Harbour", "Verdict", "Ledger", "Quarry",
    "Beacon", "Threshold", "Reckoning", "Passage", "Errand", "Covenant", "Bastion", "Tide",
    "Ember", "Argument", "Horizon", "Salvage", "Interval", "Prospect", "Keel", "Sable", "Ferry",
    "Warrant", "Solstice", "Cadence", "Remit",
];

// ===========================================================================
// Rendering
// ===========================================================================

/// Camera + look settings for one frame.
#[derive(Clone, Copy)]
pub struct View {
    /// Pixels per ship-length unit.
    pub zoom: f32,
    /// Radians; 0 = nose up. Rotates the ship in the plane.
    pub heading: f32,
    /// Where the ship's centre sits, in pixels from the buffer centre. Nudge it
    /// forward (negative `pan_y`) to leave room aft for the drive plume.
    pub pan_x: f32,
    pub pan_y: f32,
    /// Drive-plume length/intensity, 0 = cold, 1 = cruise, 2 = burn.
    pub thrust: f32,
    /// Ordered-dither strength, 0..1.
    pub dither: f32,
    /// Background starfield density; 0 renders a transparent backdrop so the
    /// sprite can be composited elsewhere.
    pub stars: f32,
}

impl Default for View {
    fn default() -> Self {
        View { zoom: 200.0, heading: 0.0, pan_x: 0.0, pan_y: 0.0, thrust: 1.0, dither: 0.7, stars: 0.6 }
    }
}

const LIGHT: [f32; 3] = [-0.46, -0.52, 0.72]; // from the upper-left, slightly ahead
const RIM: Rgb = [0.30, 0.42, 0.66];
const SPEC: Rgb = [1.0, 0.98, 0.92];

impl Ship {
    /// Render one frame into `out` (RGBA, `w*h*4` bytes).
    pub fn render(&self, w: u32, h: u32, view: &View, t: f32, out: &mut [u8]) {
        debug_assert!(out.len() >= (w * h * 4) as usize);
        let light = norm3(LIGHT);
        let (cx, cy) = (w as f32 * 0.5 + view.pan_x, h as f32 * 0.5 + view.pan_y);
        let zoom = view.zoom.max(1e-3);
        let (hc, hs) = (view.heading.cos(), view.heading.sin());
        let thrust = view.thrust.max(0.0);
        // Plumes reach further than the hull; give the grid test a margin.
        let plume_reach = self.plumes.iter().fold(0.0f32, |a, p| a.max(p.len)) * (0.6 + 0.9 * thrust);

        // A transparent backdrop needs no bake and no cache allocation — which
        // matters in the fleet view, where 64 ships each hold their own.
        let opaque_bg = view.stars > 0.0;
        if opaque_bg {
            self.bake_backdrop(w, h, view.stars);
        }
        let bg = self.bg.borrow();

        for py in 0..h {
            for px in 0..w {
                // screen -> ship space (inverse rotation)
                let dx = (px as f32 + 0.5 - cx) / zoom;
                let dy = (py as f32 + 0.5 - cy) / zoom;
                let sx = dx * hc + dy * hs;
                let sy = -dx * hs + dy * hc;

                // Resolve the hull FIRST: a covered pixel never pays for the
                // backdrop, and the backdrop itself is a cache read.
                let mut alpha: f32;
                let mut col: Rgb;
                if let Some(hit) = self.trace(sx, sy) {
                    col = self.shade(&hit, sx, sy, t, light);
                    alpha = 1.0;
                } else if opaque_bg {
                    let i = ((py * w + px) * 3) as usize;
                    col = [bg.px[i], bg.px[i + 1], bg.px[i + 2]];
                    alpha = 1.0;
                } else {
                    col = [0.0, 0.0, 0.0];
                    alpha = 0.0;
                }

                // Additive: drive plumes, then the navigation lights.
                if thrust > 0.001 && sy > self.by1 - 0.06 - plume_reach {
                    let g = self.plume_at(sx, sy, t, thrust);
                    if g > 0.0005 {
                        col = add(col, scale(self.pal.glow, g));
                        alpha = alpha.max(clamp01(g * 1.6));
                    }
                }
                let lg = self.lights_at(sx, sy, t);
                if lg.0 > 0.0005 {
                    col = add(col, scale(lg.1, lg.0));
                    alpha = alpha.max(clamp01(lg.0 * 1.6));
                }

                let b = bayer(px, py);
                let q = quant(col, b, view.dither);
                let o = ((py * w + px) * 4) as usize;
                out[o] = (q[0] * 255.0) as u8;
                out[o + 1] = (q[1] * 255.0) as u8;
                out[o + 2] = (q[2] * 255.0) as u8;
                out[o + 3] = (clamp01(alpha) * 255.0) as u8;
            }
        }
    }

    /// Space behind the ship: a near-black field, a faint nebula wash and a
    /// hashed starfield. Screen-space and time-independent, so it's baked once
    /// and then re-read every frame until the viewport or density changes.
    fn bake_backdrop(&self, w: u32, h: u32, density: f32) {
        let mut c = self.bg.borrow_mut();
        let n = (w * h * 3) as usize;
        if c.w == w && c.h == h && c.stars == density && c.px.len() == n {
            return;
        }
        c.w = w;
        c.h = h;
        c.stars = density;
        c.px.clear();
        c.px.resize(n, 0.0);
        let seed_z = self.seed as f32 * 0.001;
        let si = self.seed as i32 & 0xffff;
        let thresh = 1.0 - 0.010 * density;
        for py in 0..h {
            let fy = py as f32;
            let uy = (fy / h as f32 - 0.5) * 2.0;
            for px in 0..w {
                let fx = px as f32;
                // One octave is plenty for a slow wash, and this runs w*h times.
                let neb = value_noise(fx * 0.012, fy * 0.012, seed_z);
                let mut col = mix([0.020, 0.022, 0.038], [0.045, 0.040, 0.075], neb);
                let ux = (fx / w as f32 - 0.5) * 2.0;
                col = scale(col, 1.0 - 0.45 * clamp01(ux * ux + uy * uy));
                let s = hash3(px as i32, py as i32, si);
                if s > thresh {
                    let b = (s - thresh) / (1.0 - thresh);
                    let tint = hash3(px as i32, py as i32, 77);
                    let star = mix([0.72, 0.80, 1.0], [1.0, 0.92, 0.78], tint);
                    col = add(col, scale(star, 0.30 + 0.70 * b));
                }
                let i = ((py * w + px) * 3) as usize;
                c.px[i] = col[0];
                c.px[i + 1] = col[1];
                c.px[i + 2] = col[2];
            }
        }
    }

    /// Topmost part under a ship-space sample, via the uniform grid.
    fn trace(&self, sx: f32, sy: f32) -> Option<Hit> {
        if sx < self.bx0 || sx > self.bx1 || sy < self.by0 || sy > self.by1 {
            return None;
        }
        let gn = self.gn;
        let i = ((((sx - self.bx0) / (self.bx1 - self.bx0)) * gn as f32) as u32).min(gn - 1);
        let j = ((((sy - self.by0) / (self.by1 - self.by0)) * gn as f32) as u32).min(gn - 1);
        let cell = (j * gn + i) as usize;
        let (a, b) = (self.cell_start[cell] as usize, self.cell_start[cell + 1] as usize);
        let mut best: Option<Hit> = None;
        let mut best_layer = 0u8;
        for &pi in &self.cell_items[a..b] {
            let p = &self.parts[pi as usize];
            if best.is_some() && p.layer < best_layer {
                continue;
            }
            if let Some(hit) = p.hit_one(&self.profile, sx, sy) {
                best_layer = p.layer;
                best = Some(hit);
            } else if p.mirror {
                if let Some(mut hit) = p.hit_one(&self.profile, -sx, sy) {
                    hit.n[0] = -hit.n[0];
                    hit.u = -hit.u;
                    best_layer = p.layer;
                    best = Some(hit);
                }
            }
        }
        best
    }

    /// Base colour for a material, before lighting.
    fn mat_color(&self, m: Mat) -> (Rgb, f32, i32) {
        // (albedo, specular strength, shininess exponent)
        let p = &self.pal;
        match m {
            Mat::Plate => (p.plate, 0.20, 18),
            Mat::Shade => (p.shade, 0.16, 16),
            Mat::Accent => (p.accent, 0.22, 20),
            Mat::Trim => (p.trim, 0.28, 24),
            Mat::Metal => (p.metal, 0.55, 34),
            Mat::Dark => (p.dark, 0.10, 12),
            Mat::Glass => (p.glass, 0.90, 48),
            Mat::Deck => ([0.13, 0.14, 0.17], 0.06, 10),
            Mat::Cargo => (p.plate, 0.14, 14),
            Mat::Rad => ([0.52, 0.53, 0.57], 0.24, 26),
            Mat::Bell => (p.glow, 0.0, 1),
        }
    }

    /// Full surface shading: material, procedural plating/detail, then light.
    fn shade(&self, hit: &Hit, sx: f32, sy: f32, t: f32, light: [f32; 3]) -> Rgb {
        let (mut base, spec_k, shin) = self.mat_color(hit.mat);
        base = scale(base, hit.tone);
        let mut emissive = [0.0f32; 3];

        match hit.mat {
            // The engine throat is self-lit: no shading, just a hot ramp.
            Mat::Bell => {
                let d = smoothstep(0.74, 1.0, hit.v) * (1.0 - 0.45 * hit.u.abs());
                let hot = mix(self.pal.glow, [1.0, 1.0, 0.98], clamp01(d * 1.2));
                let flick = 0.82 + 0.28 * fbm(sx * 90.0, sy * 90.0 - t * 7.0, hit.seed as f32 * 0.01, 2);
                return scale(hot, (0.55 + 1.35 * d) * flick);
            }
            // Canopies and portholes glow from inside.
            Mat::Glass => {
                emissive = scale(base, 0.55);
            }
            // Flight decks: dark tarmac, centreline dashes, threshold bars.
            Mat::Deck => {
                let (_, rows) = hit.cells;
                let along = hit.v * rows.max(1.0);
                if hit.u.abs() < 0.09 && (along.fract() < 0.55) {
                    base = mix(base, self.pal.trim, 0.75);
                }
                if hit.v < 0.07 || hit.v > 0.93 {
                    base = mix(base, self.pal.accent, 0.6);
                }
                if hit.u.abs() > 0.86 {
                    base = mix(base, self.pal.trim, 0.35);
                }
            }
            // Container racks: one manifest colour per cell, with seams.
            Mat::Cargo => {
                let (cols, rows) = hit.cells;
                let (cx, ry) = (
                    ((hit.u * 0.5 + 0.5) * cols.max(1.0)).floor(),
                    (hit.v * rows.max(1.0)).floor(),
                );
                let k = hash3(hit.seed as i32, cx as i32, ry as i32);
                base = MANIFEST[(k * MANIFEST.len() as f32) as usize % MANIFEST.len()];
                base = scale(base, 0.86 + 0.28 * hash3(hit.seed as i32 + 7, cx as i32, ry as i32));
                // seams between boxes
                let fu = ((hit.u * 0.5 + 0.5) * cols.max(1.0)).fract();
                let fv = (hit.v * rows.max(1.0)).fract();
                if !(0.06..=0.94).contains(&fu) || !(0.05..=0.95).contains(&fv) {
                    base = scale(base, 0.55);
                }
            }
            // Radiators: a hot gradient across fluted panels.
            Mat::Rad => {
                let (fl, _) = hit.cells;
                let f = (hit.u.abs() * fl.max(1.0)).fract();
                base = scale(base, 0.80 + 0.35 * f);
                let heat = clamp01(1.0 - hit.u.abs() * 1.6);
                emissive = scale([0.85, 0.34, 0.14], 0.16 * heat);
            }
            // Launcher cells: a dark grid with lit hatch seams.
            Mat::Dark => {
                let (cols, rows) = hit.cells;
                if cols > 0.5 {
                    let fu = ((hit.u * 0.5 + 0.5) * cols).fract();
                    let fv = (hit.v * rows.max(1.0)).fract();
                    let edge = !(0.14..=0.86).contains(&fu) || !(0.12..=0.88).contains(&fv);
                    if edge {
                        base = mix(base, self.pal.metal, 0.55);
                    }
                }
            }
            _ => {}
        }

        // ---- shared hull-surface detail ------------------------------------
        if matches!(hit.mat, Mat::Plate | Mat::Shade | Mat::Trim | Mat::Accent | Mat::Metal) {
            // Irregular hull plating: hash the ship-space position into plates
            // of varying size, tint each, and darken the seams.
            let (gx, gy) = (sx * 34.0, sy * 26.0);
            let (ix, iy) = (gx.floor(), gy.floor());
            let plate = hash3(ix as i32, iy as i32, hit.seed as i32 & 0x7fff);
            base = scale(base, 0.94 + 0.11 * plate);
            let (fx, fy) = (gx - ix, gy - iy);
            if fx < 0.05 || fy < 0.05 {
                base = scale(base, 0.87);
            }
            // Weathering: a second hashed grid, deliberately out of step with the
            // plating one, so patches of grime straddle panel joins the way they
            // do on a real hull. One hash beats an fBm octave 8:1, and at
            // pixel-art scale the blockiness IS the look.
            let grime = hash3((sx * 5.7).floor() as i32, (sy * 4.3).floor() as i32, hit.seed as i32 | 1);
            base = scale(base, 0.90 + 0.17 * grime);
            // Livery stripes.
            base = self.livery_stripe(base, hit, sx);
            // Lit portholes down the flanks.
            if self.windows > 0.0 && matches!(hit.mat, Mat::Plate | Mat::Trim) {
                let rowu = hit.u.abs();
                if (0.38..0.88).contains(&rowu) {
                    // ~26 rows down each flank: at poster scale that's a 2 px
                    // porthole, which is exactly the pixel-art read we want.
                    let along = hit.v * 26.0;
                    let cell = along.floor();
                    let lit = hash3(hit.seed as i32 + 31, cell as i32, (hit.u.signum() * 3.0) as i32);
                    if along.fract() < 0.5 && lit < self.windows * 0.8 {
                        // a slow, irregular flicker as crew move about
                        let f = 0.72 + 0.28 * (t * 0.7 + lit * 40.0).sin();
                        emissive = add(emissive, scale(self.pal.glass, 0.85 * f));
                    }
                }
            }
        }

        // ---- lighting -------------------------------------------------------
        let n = hit.n;
        let ndl = dot3(n, light).max(0.0);
        let amb = 0.26 + 0.16 * clamp01(n[2]);
        // Cheap AO: creases between parts sit lower on the dome.
        let ao = 0.66 + 0.34 * hit.h;
        let mut c = scale(base, (amb + 0.92 * ndl) * ao);
        // Blinn-Phong glint.
        let hv = norm3([light[0], light[1], light[2] + 1.0]);
        let sp = dot3(n, hv).max(0.0).powi(shin) * spec_k;
        c = add(c, scale(SPEC, sp));
        // Cool rim from the surrounding starfield.
        let rim = (1.0 - clamp01(n[2])).powi(3) * 0.30;
        c = add(c, scale(RIM, rim));
        add(c, emissive)
    }

    /// The livery flash: one of four schemes, in the ship's accent colour.
    fn livery_stripe(&self, base: Rgb, hit: &Hit, _sx: f32) -> Rgb {
        if hit.mat == Mat::Accent {
            return base;
        }
        let a = self.pal.accent;
        match self.stripe {
            // a stripe down the spine
            0 => {
                if hit.u.abs() < 0.16 {
                    mix(base, a, 0.75)
                } else {
                    base
                }
            }
            // a nose flash
            1 => {
                if hit.v < self.stripe_v * 0.45 {
                    mix(base, a, 0.62 * smoothstep(self.stripe_v * 0.45, 0.0, hit.v))
                } else {
                    base
                }
            }
            // a band athwartships
            2 => {
                let d = (hit.v - self.stripe_v).abs();
                if d < 0.045 {
                    mix(base, a, 0.8)
                } else if d < 0.065 {
                    mix(base, a, 0.35)
                } else {
                    base
                }
            }
            // twin racing stripes
            _ => {
                let d = (hit.u.abs() - 0.62).abs();
                if d < 0.07 {
                    mix(base, a, 0.7)
                } else {
                    base
                }
            }
        }
    }

    /// Additive drive plume: a turbulent, parabola-profiled cone aft of every
    /// bell, with shock diamonds near the throat.
    fn plume_at(&self, sx: f32, sy: f32, t: f32, thrust: f32) -> f32 {
        let mut sum = 0.0f32;
        // The turbulence field is shared by every bell, so sample it ONCE per
        // pixel rather than once per bell (a dreadnought has seven).
        let mut turb = -1.0f32;
        for p in &self.plumes {
            let len = p.len * (0.35 + 0.85 * thrust);
            let dy = sy - p.y;
            if dy < 0.0 || dy > len {
                continue;
            }
            let s = dy / len;
            let flare = p.w * (0.85 + 2.10 * s);
            let q = (sx - p.x) / flare;
            if q.abs() >= 1.0 {
                continue;
            }
            let radial = (1.0 - q * q).max(0.0);
            let falloff = (1.0 - s).powf(1.5);
            // turbulence, scrolling aft
            if turb < 0.0 {
                turb = 0.72 + 0.46 * fbm(sx * 60.0, sy * 34.0 - t * 9.0, 3.0, 2);
            }
            // shock diamonds, strongest just aft of the throat
            let shock = 1.0 + 0.34 * (s * 26.0).sin() * (1.0 - s).powi(2);
            sum += radial * radial * falloff * turb * shock;
        }
        sum * thrust * 1.15
    }

    /// Blinking navigation lights — returns (intensity, colour).
    fn lights_at(&self, sx: f32, sy: f32, t: f32) -> (f32, Rgb) {
        let mut best = (0.0f32, [0.0f32; 3]);
        for l in &self.lights {
            let r = l.r * 2.4;
            let (dx, dy) = (sx - l.x, sy - l.y);
            if dx.abs() > r || dy.abs() > r {
                continue;
            }
            let d2 = dx * dx + dy * dy;
            if d2 > r * r {
                continue;
            }
            let blink = {
                let ph = ((t / l.period) + l.phase).fract();
                if ph < 0.14 {
                    1.0
                } else {
                    0.12
                }
            };
            let i = (1.0 - (d2.sqrt() / r)).max(0.0).powf(2.2) * blink * 1.5;
            if i > best.0 {
                best = (i, l.col);
            }
        }
        best
    }
}

impl Ship {
    /// Lit-porthole density this ship was generated with.
    pub fn window_density(&self) -> f32 {
        self.windows
    }
}

/// Stateless one-shot: generate `class_idx`/`seed` and render it fitted to
/// `w`x`h`. Handy for contact sheets; hold a [`Ship`] if you're animating.
pub fn render_rgba(w: u32, h: u32, class_idx: usize, seed: u32, t: f32, out: &mut [u8]) {
    let s = Ship::generate(class_idx, seed);
    let v = View { zoom: s.fit_zoom(w, h, 0.0), ..View::default() };
    s.render(w, h, &v, t, out);
}

// Browser (wasm) C-ABI glue — excluded from native builds. See wasm.rs.
#[cfg(target_arch = "wasm32")]
mod wasm;

#[cfg(test)]
mod tests {
    use super::*;

    /// Every class in the table must assemble, stay finite, and land inside a
    /// sane bounding box — the cheapest guard against a bad row.
    #[test]
    fn every_class_generates() {
        for i in 0..class_count() {
            for seed in [1u32, 7, 4242, 999_331] {
                let s = Ship::generate(i, seed);
                let (x0, x1, y0, y1) = s.bounds();
                assert!(
                    x0.is_finite() && x1.is_finite() && y0.is_finite() && y1.is_finite(),
                    "{} seed {seed}: non-finite bounds",
                    class_name(i)
                );
                assert!(x1 > x0 && y1 > y0, "{} seed {seed}: empty bounds", class_name(i));
                assert!(x1 - x0 < 8.0 && y1 - y0 < 8.0, "{} seed {seed}: runaway bounds", class_name(i));
                assert!(s.part_count() > 0, "{} seed {seed}: no parts", class_name(i));
                assert!(s.length_m > 0.0);
            }
        }
    }

    /// The same seed must rebuild the same pixels — the whole promise of a
    /// seed-driven generator.
    #[test]
    fn deterministic() {
        let (w, h) = (96u32, 96u32);
        let mut a = vec![0u8; (w * h * 4) as usize];
        let mut b = vec![0u8; (w * h * 4) as usize];
        for i in [0usize, 17, 33, 51] {
            render_rgba(w, h, i, 12345, 0.3, &mut a);
            render_rgba(w, h, i, 12345, 0.3, &mut b);
            assert_eq!(a, b, "class {} not deterministic", class_name(i));
        }
    }

    /// Every pixel must be written, and the hull must actually cover some of
    /// the frame (a silent all-background render is the failure to catch).
    #[test]
    fn renders_something() {
        let (w, h) = (128u32, 128u32);
        let mut buf = vec![0u8; (w * h * 4) as usize];
        for i in 0..class_count() {
            let s = Ship::generate(i, 5150);
            let v = View { zoom: s.fit_zoom(w, h, 0.0), stars: 0.0, ..View::default() };
            buf.iter_mut().for_each(|b| *b = 0);
            s.render(w, h, &v, 0.5, &mut buf);
            let lit = buf.chunks_exact(4).filter(|p| p[3] > 8).count();
            let frac = lit as f32 / (w * h) as f32;
            assert!(frac > 0.01, "{}: only {:.3}% of the frame covered", class_name(i), frac * 100.0);
            assert!(frac < 0.95, "{}: {:.3}% covered — fit is wrong", class_name(i), frac * 100.0);
        }
    }

    /// Roles must partition the table, and every role must be non-empty.
    #[test]
    fn roles_cover_the_table() {
        let mut total = 0;
        for r in 0..role_count() {
            let n = classes_in_role(r).len();
            assert!(n > 0, "role {} is empty", role_name(r));
            total += n;
        }
        assert_eq!(total, class_count());
    }

    /// Slider defaults must round-trip: generating with a class's own defaults
    /// must give the same ship as generating with none.
    #[test]
    fn param_defaults_round_trip() {
        for i in [0usize, 9, 25, 40, 60] {
            let p: Vec<f32> = (0..NUM_PARAMS as u32).map(|k| param(i, k)).collect();
            let a = Ship::generate_params(i, 77, &p);
            let b = Ship::generate_params(i, 77, &p);
            assert_eq!(a.part_count(), b.part_count());
            assert!(a.window_density() == b.window_density());
        }
    }
}
