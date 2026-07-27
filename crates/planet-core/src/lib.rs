//! planet-core — the single source of truth for procedural planet generation.
//!
//! Pure math over the dependency-free `noise-core` + `dither-core` rlibs.
//! Produces raw RGBA bytes; callers wrap those however they like (the `planet`
//! crate turns them into GIFs/PNGs with the `image` crate and exposes them to a
//! canvas over the C ABI; `solar` composites them into a system view).
//!
//! A planet TYPE is a [`PType`] row (palette + thresholds + flags). Four... five
//! base algorithms render everything: Terrestrial, Cratered, Banded, Emissive,
//! Cloudy. Rings and specular "glare" are reusable modifiers. Same inputs =>
//! same planet. The "3D" is per-pixel sphere math: rotate the surface point
//! around Y and sample 3D noise there, shade against a light direction.
//!
//! ONE shader, two framings (see [`Frame`]):
//!   • **hero** — [`render_rgba`] & friends: the planet fills a square frame over
//!     a starfield, lit by a fixed key light, optionally with orbiting moons.
//!     This is what the `planet` demo shows.
//!   • **scene** — [`render_tile`]: the same planet cut out on transparency, sized
//!     to its disc and lit from an arbitrary direction, ready for a compositor to
//!     blit. This is what `solar` puts in orbit around its star.

use std::cell::RefCell;
use std::f32::consts::{FRAC_PI_2, PI, TAU};
use std::rc::Rc;

pub use scene_core::Tile;

// ---------------------------------------------------------------------------
// Low-level primitives (noise, color/ramp helpers, ordered dither) now live in
// the shared `noise-core` / `dither-core` rlibs — byte-for-byte identical to the
// copies that used to live here. Imported below; `seed_offsets` keeps a thin
// local wrapper (span 256) so call sites and numeric output are unchanged.
// ---------------------------------------------------------------------------

use dither_core::{bayer, quant};
use noise_core::{
    clamp01, contrast, cycle3, fbm, fbm_warp_inner, hash3, lerp, mix, ramp, smoothstep, worley, Rgb,
};

// ---------------------------------------------------------------------------
// Feature switches
// ---------------------------------------------------------------------------
//
// The per-type sliders already reach most of the shader — `clouds`, `specular`,
// `spot`, `aurora`, `lightning`, `storm_cells` and `caps` are all gated on
// `> 0.0`, so zeroing one switches that feature off. These are the pieces a
// parameter cannot reach: parts of a layer rather than a whole one, framing
// furniture, and the two optimizations. A SET bit means the feature is ON.

/// The cloud deck's self-shadow, independent of the cloud colour above it.
pub const F_CLOUD_SHADOW: u32 = 1;
/// The atmosphere rim glow at the limb.
pub const F_ATMO: u32 = 2;
/// The crisp 1px dark outline around the disc.
pub const F_RIM: u32 = 4;
/// The hashed starfield behind the planet (hero framing only).
pub const F_STARFIELD: u32 = 8;
/// OPTIMIZATION: skip the fine octaves and the cloud deck past the terminator.
pub const F_NIGHT_LOD: u32 = 16;
/// OPTIMIZATION: run a domain warp's displacement fields at 2 octaves.
pub const F_CHEAP_WARP: u32 = 32;
/// OPTIMIZATION: freeze the cloud deck and read it from a baked map (see
/// [`CloudMap`]). Costs the billowing and the churning storm cells; the deck
/// still rotates over the surface at its own rate.
///
/// Deliberately NOT in [`F_ALL`]: it is the one switch here that changes the
/// picture rather than the pixel budget, so the native generators keep the
/// animated deck and `out/` stays byte-identical. The web demos opt in.
pub const F_BAKED_CLOUDS: u32 = 64;
/// OPTIMIZATION: bake the base surface albedo into the sphere map as well.
///
/// Only the two families that are pure functions of a direction on the sphere —
/// `Terrestrial` and `Cratered`, 15 of the 26 types. A gas giant's zonal jets
/// and a lava world's molten flow advect with `angle` and stay live.
///
/// Also covers `Emissive`, whose 6-octave rock field is static — only the
/// 3-octave flow that lights it advects, and that stays live, so a lava world
/// keeps flowing at full speed.
///
/// Like [`F_BAKED_CLOUDS`], deliberately outside [`F_ALL`].
pub const F_BAKED_SURFACE: u32 = 128;
/// Restore the cloud deck's billowing on top of [`F_BAKED_CLOUDS`], by baking
/// the deck at several points along its morph cycle and interpolating.
///
/// The morph translates the noise domain in y **and** z — the field evolving,
/// not moving — so no lookup offset represents it and no single map holds it.
/// Discretizing that axis does: `MORPH_PHASES` maps across the cycle, indexed by
/// the morph *value* rather than by time, since it oscillates rather than
/// advancing. Costs one extra tap per plane and `MORPH_PHASES`x the memory and
/// bake, and buys back the half of the deck's life that freezing took.
///
/// The storm swirl stays frozen: it runs on its own cycle, so restoring it too
/// would need the product of the two axes rather than the sum.
pub const F_MORPH_LUT: u32 = 512;
/// How many points along the morph cycle [`F_MORPH_LUT`] bakes.
///
/// The morph spans ±0.6 of a lattice cell at the base octave, which is ±4.8 at
/// the fourth — so adjacent phases are well correlated coarsely and independent
/// finely, and the interpolation between them reads as cloud forming and
/// dissipating rather than sliding. Six is where that still looks continuous;
/// fewer and the dissolve steps.
const MORPH_PHASES: u8 = 6;
/// Half-width of the morph cycle: `angle.sin() * MORPH_SPAN`.
const MORPH_SPAN: f32 = 0.6;

/// OPTIMIZATION: bake `Base::Banded`, re-expressing its zonal drift as a
/// rotation in longitude instead of a shear of the noise domain.
///
/// The drift `angle · 0.16 · sin(lat · bands / 2)` is added to the sample's *x*
/// today, which slides the field through the sphere and so cannot be a lookup
/// offset. As a longitude rate it stays on the sphere, and animating the bands
/// costs one subtraction from the texture coordinate. The bake is exact under
/// that model: `band` is a function of the warp and of `y`, and a shift in
/// longitude leaves `y` alone.
///
/// Its own bit, and outside [`F_ALL`], because unlike the others it changes what
/// the motion *is* — the bands counter-rotate rather than shearing past each
/// other. That is arguably what the code always meant to do, but it does not
/// look the same.
pub const F_BAKED_BANDS: u32 = 256;
/// Everything on — what every caller but the demo's ablation panel wants.
pub const F_ALL: u32 = 63;

/// Octaves for a domain warp's three *displacement* fields.
///
/// `fbm_warp` runs its inner fields at the same count as the outer one, but they
/// only bend the outer field's domain — their fine octaves are nearly invisible
/// in the result and cost full price. Two octaves is the measured knee: the warp
/// kernel gets 36-44% cheaper and the field moves by a mean of 0.019, against a
/// dither step of 0.045. Below 2 the marbling starts to straighten out.
const WARP_INNER: u32 = 2;

/// Inner-field octaves for a warp whose outer count is `outer`.
#[inline(always)]
fn warp_inner(feat: u32, outer: u32) -> u32 {
    if feat & F_CHEAP_WARP != 0 { WARP_INNER } else { outer }
}

// ---------------------------------------------------------------------------
// Planet type table
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Base {
    Terrestrial,
    Cratered,
    Banded,
    Emissive,
    Cloudy,
}

#[derive(Clone, Copy)]
pub struct PType {
    name: &'static str,
    base: Base,
    freq: f32,
    contrast: f32,
    ridged: bool,
    stops: &'static [(f32, Rgb)],
    clouds: f32,
    caps: f32,
    atmo: Rgb,
    light: Rgb,
    dark: Rgb,
    bands: f32,
    turb: f32,
    rock: Rgb,
    glow_lo: Rgb,
    glow_hi: Rgb,
    glow_e0: f32,
    glow_e1: f32,
    rings: bool,
    ring_inner: f32,
    ring_outer: f32,
    ring_col: Rgb,
    radius_scale: f32,
    specular: f32,
    shininess: f32,
    spec_albedo: f32, // how much specular is scaled by local albedo (dark = less glare)
    // weather
    spot: f32,        // great-spot cyclone intensity (banded worlds)
    lightning: f32,   // storm-flash intensity (cloudy/storm worlds)
    aurora: f32,      // polar aurora intensity
    storm_cells: f32, // rotating hurricane swirls in the cloud layer
}

const fn base() -> PType {
    PType {
        name: "",
        base: Base::Terrestrial,
        freq: 2.0,
        contrast: 1.8,
        ridged: false,
        stops: &[],
        clouds: 0.0,
        caps: 0.0,
        atmo: [0.0; 3],
        light: [0.6, 0.6, 0.62],
        dark: [0.2, 0.2, 0.22],
        bands: 11.0,
        turb: 0.6,
        rock: [0.14, 0.08, 0.07],
        glow_lo: [1.0, 0.42, 0.06],
        glow_hi: [1.0, 0.92, 0.35],
        glow_e0: 0.44,
        glow_e1: 0.66,
        rings: false,
        ring_inner: 1.35,
        ring_outer: 2.10,
        ring_col: [0.80, 0.72, 0.58],
        radius_scale: 1.0,
        specular: 0.0,
        shininess: 24.0,
        spec_albedo: 0.6, // rocky default: glare follows the surface brightness
        spot: 0.0,
        lightning: 0.0,
        aurora: 0.0,
        storm_cells: 0.0,
    }
}

// -- color ramps --
const TERRAN: &[(f32, Rgb)] = &[
    (0.42, [0.08, 0.16, 0.36]), (0.48, [0.13, 0.30, 0.55]), (0.50, [0.78, 0.73, 0.52]),
    (0.62, [0.28, 0.54, 0.26]), (0.74, [0.16, 0.38, 0.18]), (0.86, [0.45, 0.40, 0.34]),
    (1.01, [0.90, 0.90, 0.92]),
];
const OCEAN: &[(f32, Rgb)] = &[
    (0.55, [0.05, 0.13, 0.33]), (0.66, [0.10, 0.27, 0.51]), (0.68, [0.76, 0.70, 0.50]),
    (0.74, [0.30, 0.52, 0.30]), (1.01, [0.19, 0.42, 0.22]),
];
const ARCHIPELAGO: &[(f32, Rgb)] = &[
    (0.47, [0.07, 0.22, 0.44]), (0.52, [0.17, 0.50, 0.64]), (0.55, [0.86, 0.80, 0.58]),
    (0.63, [0.34, 0.60, 0.34]), (1.01, [0.22, 0.46, 0.26]),
];
const DESERT: &[(f32, Rgb)] = &[
    (0.40, [0.52, 0.32, 0.19]), (0.52, [0.78, 0.55, 0.32]), (0.66, [0.87, 0.69, 0.43]),
    (0.80, [0.93, 0.82, 0.57]), (1.01, [0.72, 0.50, 0.34]),
];
const SWAMP: &[(f32, Rgb)] = &[
    (0.46, [0.15, 0.20, 0.11]), (0.50, [0.30, 0.29, 0.15]), (0.62, [0.25, 0.42, 0.16]),
    (0.78, [0.15, 0.33, 0.13]), (1.01, [0.31, 0.39, 0.20]),
];
const IRON: &[(f32, Rgb)] = &[
    (0.40, [0.28, 0.11, 0.07]), (0.55, [0.55, 0.22, 0.12]), (0.70, [0.73, 0.35, 0.18]),
    (0.85, [0.60, 0.40, 0.30]), (1.01, [0.86, 0.56, 0.36]),
];
const ICE: &[(f32, Rgb)] = &[
    (0.30, [0.83, 0.91, 0.99]), (0.55, [0.68, 0.80, 0.93]), (0.75, [0.50, 0.66, 0.86]),
    (1.01, [0.34, 0.51, 0.78]),
];
const SAVANNA: &[(f32, Rgb)] = &[
    (0.42, [0.55, 0.45, 0.20]), (0.55, [0.78, 0.68, 0.32]), (0.70, [0.62, 0.62, 0.28]),
    (0.82, [0.40, 0.52, 0.22]), (1.01, [0.68, 0.60, 0.40]),
];
const GAIA: &[(f32, Rgb)] = &[
    (0.35, [0.10, 0.28, 0.12]), (0.55, [0.18, 0.42, 0.18]), (0.72, [0.30, 0.55, 0.24]),
    (0.88, [0.45, 0.62, 0.30]), (1.01, [0.75, 0.80, 0.62]),
];
const TUNDRA: &[(f32, Rgb)] = &[
    (0.45, [0.80, 0.84, 0.90]), (0.58, [0.66, 0.70, 0.76]), (0.70, [0.45, 0.44, 0.42]),
    (0.82, [0.35, 0.33, 0.30]), (1.01, [0.72, 0.76, 0.82]),
];
const ALPINE: &[(f32, Rgb)] = &[
    (0.40, [0.14, 0.22, 0.30]), (0.52, [0.24, 0.34, 0.22]), (0.66, [0.40, 0.36, 0.30]),
    (0.80, [0.60, 0.58, 0.55]), (1.01, [0.95, 0.96, 1.00]),
];
const OBSIDIAN: &[(f32, Rgb)] = &[
    (0.45, [0.10, 0.09, 0.13]), (0.65, [0.16, 0.14, 0.20]), (0.82, [0.26, 0.23, 0.32]),
    (1.01, [0.40, 0.36, 0.50]),
];
const CHROME: &[(f32, Rgb)] = &[
    (0.40, [0.35, 0.37, 0.42]), (0.60, [0.55, 0.58, 0.63]), (0.80, [0.75, 0.78, 0.83]),
    (1.01, [0.92, 0.94, 0.98]),
];

/// The 26 planet types. Adding a type = adding a row, in ONE place.
/// Glare: low shininess = broad wet/icy glare; high = tight metal/glass glint.
pub const TYPES: &[PType] = &[
    // family A — terrestrial (water worlds get a broad wet glint)
    PType { name: "terran", base: Base::Terrestrial, freq: 2.0, contrast: 2.1, stops: TERRAN, clouds: 0.85, caps: 0.9, atmo: [0.30, 0.45, 0.65], specular: 0.22, shininess: 8.0, spec_albedo: 0.0, aurora: 0.8, storm_cells: 0.3, ..base() },
    PType { name: "ocean", base: Base::Terrestrial, freq: 2.2, contrast: 1.7, stops: OCEAN, clouds: 0.7, caps: 0.7, atmo: [0.25, 0.42, 0.66], specular: 0.32, shininess: 7.0, spec_albedo: 0.0, storm_cells: 0.6, ..base() },
    PType { name: "archipelago", base: Base::Terrestrial, freq: 4.0, contrast: 1.6, stops: ARCHIPELAGO, clouds: 0.5, caps: 0.3, atmo: [0.24, 0.48, 0.62], specular: 0.26, shininess: 8.0, spec_albedo: 0.0, storm_cells: 0.25, ..base() },
    PType { name: "desert", base: Base::Terrestrial, freq: 2.4, contrast: 1.5, stops: DESERT, clouds: 0.12, caps: 0.15, atmo: [0.38, 0.28, 0.18], specular: 0.04, shininess: 24.0, ..base() },
    PType { name: "swamp", base: Base::Terrestrial, freq: 2.6, contrast: 1.6, stops: SWAMP, clouds: 0.6, caps: 0.0, atmo: [0.24, 0.34, 0.20], specular: 0.12, shininess: 9.0, ..base() },
    PType { name: "iron", base: Base::Terrestrial, freq: 2.2, contrast: 1.9, stops: IRON, clouds: 0.0, caps: 0.1, atmo: [0.42, 0.20, 0.12], specular: 0.06, shininess: 20.0, ..base() },
    // family E — ice shell (terrestrial + ridged): noticeable icy sheen
    PType { name: "ice", base: Base::Terrestrial, freq: 2.6, contrast: 1.4, ridged: true, stops: ICE, clouds: 0.2, caps: 0.0, atmo: [0.45, 0.60, 0.85], specular: 0.45, shininess: 14.0, spec_albedo: 0.0, aurora: 1.0, ..base() },
    // family B — cratered (light=highland, dark=maria): matte dust, glare follows albedo
    PType { name: "barren", base: Base::Cratered, freq: 5.0, light: [0.55, 0.55, 0.58], dark: [0.20, 0.20, 0.23], specular: 0.0, shininess: 24.0, spec_albedo: 0.9, ..base() },
    // family C — banded (gas isn't shiny: soft, broad)
    PType { name: "gas_giant", base: Base::Banded, light: [0.86, 0.77, 0.60], dark: [0.55, 0.40, 0.28], bands: 11.0, turb: 0.6, specular: 0.05, shininess: 6.0, spot: 0.6, aurora: 0.4, ..base() },
    PType { name: "ice_giant", base: Base::Banded, light: [0.55, 0.72, 0.90], dark: [0.22, 0.38, 0.68], bands: 8.0, turb: 0.35, atmo: [0.30, 0.45, 0.70], specular: 0.08, shininess: 8.0, spec_albedo: 0.0, aurora: 0.7, ..base() },
    // family D — emissive (self-lit; little/no glare)
    PType { name: "lava", base: Base::Emissive, rock: [0.16, 0.09, 0.07], glow_lo: [1.0, 0.42, 0.06], glow_hi: [1.0, 0.92, 0.35], glow_e0: 0.44, glow_e1: 0.66, freq: 3.0, specular: 0.05, shininess: 20.0, ..base() },
    PType { name: "fungal", base: Base::Emissive, rock: [0.10, 0.10, 0.14], glow_lo: [0.15, 0.85, 0.75], glow_hi: [0.65, 0.35, 0.95], glow_e0: 0.50, glow_e1: 0.72, freq: 3.2, atmo: [0.14, 0.32, 0.34], specular: 0.0, shininess: 24.0, ..base() },
    // --- second batch ---
    PType { name: "savanna", base: Base::Terrestrial, freq: 2.2, contrast: 1.6, stops: SAVANNA, clouds: 0.25, caps: 0.10, atmo: [0.40, 0.35, 0.20], specular: 0.04, shininess: 24.0, ..base() },
    PType { name: "gaia", base: Base::Terrestrial, freq: 2.2, contrast: 1.7, stops: GAIA, clouds: 0.60, caps: 0.20, atmo: [0.30, 0.50, 0.35], specular: 0.09, shininess: 9.0, storm_cells: 0.5, ..base() },
    PType { name: "tundra", base: Base::Terrestrial, freq: 2.4, contrast: 1.6, stops: TUNDRA, clouds: 0.30, caps: 0.90, atmo: [0.50, 0.60, 0.75], specular: 0.35, shininess: 12.0, ..base() },
    PType { name: "alpine", base: Base::Terrestrial, freq: 2.6, contrast: 2.6, stops: ALPINE, clouds: 0.40, caps: 0.50, atmo: [0.40, 0.50, 0.70], specular: 0.14, shininess: 12.0, ..base() },
    PType { name: "obsidian", base: Base::Terrestrial, freq: 2.4, contrast: 1.8, stops: OBSIDIAN, clouds: 0.0, caps: 0.0, atmo: [0.20, 0.15, 0.30], specular: 0.55, shininess: 30.0, spec_albedo: 0.0, ..base() },
    PType { name: "chrome", base: Base::Terrestrial, freq: 2.2, contrast: 2.0, stops: CHROME, clouds: 0.0, caps: 0.0, atmo: [0.30, 0.35, 0.45], specular: 0.95, shininess: 32.0, spec_albedo: 0.0, ..base() },
    // more cratered — glare follows albedo (dark maria stay matte)
    PType { name: "moon", base: Base::Cratered, freq: 4.0, light: [0.62, 0.62, 0.60], dark: [0.28, 0.28, 0.30], specular: 0.0, shininess: 24.0, spec_albedo: 0.9, ..base() },
    // more banded + ringed
    PType { name: "storm_giant", base: Base::Banded, light: [0.80, 0.55, 0.45], dark: [0.45, 0.22, 0.20], bands: 9.0, turb: 1.1, specular: 0.04, shininess: 6.0, spot: 1.0, lightning: 0.5, ..base() },
    PType { name: "ringed_giant", base: Base::Banded, light: [0.82, 0.74, 0.58], dark: [0.50, 0.40, 0.30], bands: 10.0, turb: 0.5,
            rings: true, ring_inner: 1.30, ring_outer: 2.20, ring_col: [0.82, 0.74, 0.58], radius_scale: 0.50, specular: 0.05, shininess: 6.0, spot: 0.4, ..base() },
    // more emissive
    PType { name: "molten_sea", base: Base::Emissive, rock: [0.25, 0.10, 0.06], glow_lo: [1.0, 0.35, 0.05], glow_hi: [1.0, 0.85, 0.40], glow_e0: 0.30, glow_e1: 0.55, freq: 2.6, atmo: [0.30, 0.10, 0.05], specular: 0.06, shininess: 18.0, ..base() },
    PType { name: "radioactive", base: Base::Emissive, rock: [0.10, 0.14, 0.08], glow_lo: [0.40, 0.90, 0.20], glow_hi: [0.80, 1.0, 0.40], glow_e0: 0.50, glow_e1: 0.72, freq: 3.0, atmo: [0.20, 0.40, 0.10], specular: 0.0, shininess: 24.0, ..base() },
    PType { name: "crystal", base: Base::Emissive, rock: [0.15, 0.10, 0.20], glow_lo: [0.50, 0.30, 0.90], glow_hi: [0.70, 0.90, 1.0], glow_e0: 0.55, glow_e1: 0.68, freq: 3.5, atmo: [0.30, 0.25, 0.50], specular: 0.45, shininess: 30.0, spec_albedo: 0.0, ..base() },
    // family E — cloud-shrouded (soft diffuse glare)
    PType { name: "toxic", base: Base::Cloudy, light: [0.85, 0.82, 0.45], dark: [0.55, 0.60, 0.25], bands: 6.0, turb: 1.0, atmo: [0.50, 0.50, 0.20], specular: 0.06, shininess: 6.0, lightning: 0.7, ..base() },
    PType { name: "storm_shroud", base: Base::Cloudy, light: [0.85, 0.86, 0.90], dark: [0.45, 0.48, 0.55], bands: 5.0, turb: 1.2, atmo: [0.40, 0.45, 0.55], specular: 0.08, shininess: 6.0, lightning: 1.0, ..base() },
];

/// Number of planet types.
pub fn type_count() -> usize {
    TYPES.len()
}
/// Name of a planet type (wraps on out-of-range index).
pub fn type_name(i: usize) -> &'static str {
    TYPES[i % TYPES.len()].name
}
/// Index of the type called `name` (the inverse of [`type_name`]), if it exists.
/// Lets a caller name the archetypes it wants instead of hard-coding indices
/// into [`TYPES`], which would silently re-point if a row moved.
pub fn type_index(name: &str) -> Option<usize> {
    TYPES.iter().position(|t| t.name == name)
}
/// True if this type is a **giant** — a banded gas/ice world. Scene generators
/// use it to give giants a much larger body radius than a rocky world.
pub fn is_giant(i: usize) -> bool {
    TYPES[i % TYPES.len()].base == Base::Banded
}

// ---------------------------------------------------------------------------
// Surface shading
// ---------------------------------------------------------------------------

/// Bounded, decorrelated noise offsets from a seed. These MUST stay small:
/// huge sample coordinates lose f32 precision and the noise collapses into
/// horizontal bands (the "circular planet" bug with large random seeds).
fn seed_offsets(seed: u32) -> [f32; 3] {
    noise_core::seed_offsets(seed, 256.0)
}

/// A drifting spiral cyclone (great-spot) tint on a banded world, with a calm eye.
fn great_spot(col: Rgb, sx: f32, sy: f32, sz: f32, angle: f32, intensity: f32) -> Rgb {
    let spot_lat = 0.28;
    let spot_lon = 0.6 + angle.sin() * 0.18; // gently oscillates (loop-safe)
    let lon = sz.atan2(sx);
    let mut dlon = lon - spot_lon;
    while dlon > PI {
        dlon -= TAU;
    }
    while dlon < -PI {
        dlon += TAU;
    }
    let dlat = sy - spot_lat;
    // Turbulent, irregular boundary — not a clean geometric oval.
    let edge = fbm(dlon * 3.0 + sy * 4.0, dlat * 3.0, sz * 2.0, 2);
    let d = ((dlon * 1.05).powi(2) + (dlat * 2.2).powi(2)).sqrt() * (0.82 + 0.4 * edge);
    if d >= 1.0 {
        return col;
    }
    // spiral streaks that churn with time; the streaks read as the vortex, no rim.
    let swirl = (1.0 - d) * 5.0 + angle * 1.2;
    let (s, c) = swirl.sin_cos();
    let lx = dlon * c - dlat * s;
    let ly = dlon * s + dlat * c;
    let streak = fbm(lx * 8.0, ly * 8.0, sy * 2.0, 4);
    let core = smoothstep(1.0, 0.15, d) * intensity;
    let spot_col = mix([0.80, 0.36, 0.26], [0.93, 0.66, 0.46], smoothstep(0.40, 0.82, streak));
    let mut out = mix(col, spot_col, core * 0.78);
    // Recognizable hurricane eye: a small calm dark center.
    let eye = smoothstep(0.20, 0.06, d) * intensity;
    out = mix(out, [0.28, 0.11, 0.10], eye * 0.7);
    out
}

/// Shimmering polar aurora intensity (0..1) at this surface point.
fn aurora_glow(sx: f32, sy: f32, sz: f32, angle: f32) -> f32 {
    let lat = sy.abs();
    let band = smoothstep(0.55, 0.70, lat) * (1.0 - smoothstep(0.82, 0.96, lat));
    if band <= 0.0 {
        return 0.0;
    }
    let lon = sz.atan2(sx);
    // curtains: drift in longitude + shimmer over time
    let curtain = fbm(lon * 2.5 + angle * 1.5, lat * 9.0, sy * 3.0 + angle, 3);
    band * smoothstep(0.48, 0.78, curtain)
}

/// Irregular lightning flash: returns (intensity 0..1, color). Randomized in
/// occurrence, timing, intensity, size, and color; the pattern never repeats.
fn lightning_flash(sx: f32, sy: f32, angle: f32) -> (f32, Rgb) {
    const SLOTS: f32 = 13.0; // potential flash windows per rotation
    let t = angle * SLOTS / TAU;
    let slot = t.floor() as i32; // absolute index -> flashes never repeat
    let phase = t - t.floor();
    // Only some windows actually fire (~half), so the rhythm is irregular.
    if hash3(slot, 9, 5) > 0.5 {
        return (0.0, [0.0; 3]);
    }
    // Random onset within the window + a brief flicker envelope.
    let p = phase - hash3(slot, 8, 5) * 0.45;
    let env = smoothstep(0.0, 0.02, p) * (1.0 - smoothstep(0.05, 0.16, p));
    if env <= 0.0 {
        return (0.0, [0.0; 3]);
    }
    let intensity = 0.45 + hash3(slot, 7, 5) * 1.0; // random brightness
    let hx = hash3(slot, 1, 5) * 2.0 - 1.0;
    let hy = (hash3(slot, 2, 5) * 2.0 - 1.0) * 0.7;
    let radius = 0.05 + hash3(slot, 3, 5) * 0.13; // random size
    let d = ((sx - hx).powi(2) + (sy - hy).powi(2)).sqrt();
    let mag = env * intensity * smoothstep(radius, 0.0, d);
    // random color
    let hue = hash3(slot, 4, 5);
    let col = if hue < 0.42 {
        [0.75, 0.83, 1.0] // white-blue
    } else if hue < 0.66 {
        [0.82, 0.60, 1.0] // violet
    } else if hue < 0.85 {
        [0.55, 0.95, 1.0] // teal
    } else {
        [1.0, 0.90, 0.66] // warm gold
    };
    (mag, col)
}

// ---------------------------------------------------------------------------
// Level of detail
// ---------------------------------------------------------------------------

/// How much fBm detail a tile keeps.
///
/// A scene tile's noise is sampled in *disc-normalised* coordinates, so a bigger
/// tile spreads the same field over more pixels: the finest octaves shrink to
/// sub-pixel wobble and then disappear into the ordered dither. Past a point
/// they cost their full price and change nothing you can see, which is what this
/// drops. `sun-core` has done the same for the star since it was split out; the
/// thresholds here are its (`size > 200`), and the native generators never reach
/// them — solar's biggest planet tile is r≈12, moon's r≈85 — so `out/` is
/// untouched by construction.
///
/// The two fields are tuned apart, and the split is the opposite of what it
/// looks like it should be. Clouds are 61% of a `terran` frame, so cutting them
/// is where the speed is — but measured against full detail, one dropped cloud
/// octave moves 22% of the disc (mean 3.3/255) while one dropped *surface*
/// octave moves 3% (mean 1.3). The soft layer is the one you notice, because it
/// is broad and low-contrast and the eye reads its silhouette. So the surface
/// octave goes first and clouds only follow past 400px.
#[derive(Clone, Copy, PartialEq)]
struct Lod {
    surface: u32,
    cloud: u32,
}

/// Every octave, always — the hero framing and any tile below the threshold.
const LOD_FULL: Lod = Lod { surface: 0, cloud: 0 };

/// The floor used past the terminator (see `NIGHT_DIFF`).
const LOD_NIGHT: Lod = Lod { surface: 9, cloud: 9 };

// Thinning starts exactly at the geometric terminator (`diff <= 0`), where
// `shade` bottoms out at the 0.10 ambient floor and the output has ~3 of its 22
// levels left to say anything with.

impl Lod {
    fn for_size(size: u32, enabled: bool) -> Lod {
        if !enabled || size <= 200 {
            LOD_FULL
        } else if size <= 400 {
            Lod { surface: 1, cloud: 0 }
        } else {
            Lod { surface: 1, cloud: 1 }
        }
    }
    /// Octaves for the terrain/band field. Floored at 4: below that a
    /// terrestrial world loses its coastlines, not just its grain.
    #[inline(always)]
    fn surf(self, n: u32) -> u32 {
        n.saturating_sub(self.surface).max(4)
    }
    /// Octaves for the cloud field. Floored at 2 — fronts stay ragged.
    #[inline(always)]
    fn cld(self, n: u32) -> u32 {
        n.saturating_sub(self.cloud).max(2)
    }
}

// ---------------------------------------------------------------------------
// Baked cloud deck (F_BAKED_CLOUDS)
// ---------------------------------------------------------------------------
//
// The live deck costs 14 `value_noise` evaluations per pixel — a 4-octave
// domain warp for the cloud tops (3 inner fields + 1 outer = 10) plus a plain
// 4-octave field for the self-shadow (4). That is the single most expensive
// thing on a cloudy planet, ~55% of a `terran` frame.
//
// All 14 collapse into two table reads the moment the deck stops *evolving*.
// The deck already rotates at its own rate (2x the surface, so weather drifts
// across the continents); what makes it per-frame work is the billowing morph
// and the churning storm swirl, both driven by `angle`. Freeze those and the
// density becomes a fixed function of a direction on the sphere — bakeable.
//
// The map is equirectangular in (longitude, y). y rather than latitude is
// deliberate: the sphere point is (r·cos θ, y, r·sin θ) with r = √(1−y²), so a
// row of the map is exactly a circle of constant y and the vertical axis needs
// no transcendental at lookup time. It is also equal-area (Lambert), so texels
// carry uniform detail instead of piling up at the poles.
//
// Stored as `u8`. The map feeds `smoothstep(0.52, 0.70, ·)`, an 0.18-wide ramp,
// so one quantum of storage moves the result by 2.2% of the ramp — against a
// dither step of 0.045 in the output, invisible.

/// Where in its cycle the storm swirl is frozen.
///
/// Live, the eddies churn back and forth as `(angle · 0.6).sin()` — they spend
/// most of the cycle part-wound and pass through 0 (no swirl at all) twice per
/// turn. Baking the mean would straighten the cells out entirely, so this picks
/// a well-wound state instead: high enough that the vortices read as storms,
/// short of the peak where the tightest ones start to smear into rings.
const STORM_STATIC: f32 = 0.7;

/// Where the band shear is frozen.
///
/// `Base::Cloudy` shears the field along longitude by `(…).sin(angle)`, which
/// is what makes its bands churn. Unlike the swirl there is nothing to lose by
/// taking the zero of that cycle: the shear only *displaces* an already
/// domain-warped field, so at zero the bands are exactly as turbulent, they
/// just stop sliding past each other.
const SHEAR_STATIC: f32 = 0.0;

/// A frozen weather layer, equirectangular in (longitude, y) — see the notes
/// above. Two kinds of planet get one, and a plane is empty when it does not
/// apply:
///
/// * a **deck** over a solid surface (`clouds > 0`): `warp` is the
///   domain-warped density that colours the cloud tops, `dens` the plain
///   density the self-shadow reads.
/// * a **shroud** that *is* the surface (`Base::Cloudy`): `shroud` is the
///   finished band/turbulence mix factor, so the lookup lands one `mix` away
///   from the pixel's colour.
///
/// Every plane covers the whole sphere, so one map serves every angle the
/// planet will ever be seen at.
struct CloudMap {
    w: u32,
    h: u32,
    /// The deck's two fields, `phases` maps deep and phase-major: plane `k`
    /// starts at `k * w * h`. `phases == 1` is the frozen deck.
    warp: Vec<u8>,
    dens: Vec<u8>,
    phases: u8,
    /// The baked base surface, for the families whose base is one or two scalar
    /// fields rather than a colour. What they hold depends on `ct.base`, and a
    /// planet has exactly one base, so there is no ambiguity:
    ///
    /// | base | `base_a` | `base_b` |
    /// |---|---|---|
    /// | `Cloudy` | finished band/turbulence mix factor | — |
    /// | `Emissive` | the static rock field `n` | — |
    /// | `Banded` | band mix factor | fine-detail mix factor |
    ///
    /// `Banded` needs two because its two fields drift at different rates (1.0
    /// and 1.4), so one lookup offset cannot serve both.
    base_a: Vec<u8>,
    base_b: Vec<u8>,
    /// Base albedo for `Terrestrial`/`Cratered`, RGB interleaved (3 bytes/texel)
    /// — see [`F_BAKED_SURFACE`]. Interleaved rather than three planes so one
    /// lookup touches one cache line instead of three.
    surf: Vec<u8>,
}

/// Everything the baked layer depends on. `clouds` is absent on purpose — it
/// only scales the deck's opacity after the lookup, so changing it re-uses the
/// map. So is the light direction: the shadow tap moves, the field does not.
/// The `f32`s that do matter are keyed by bit pattern, since `f32` is not `Eq`.
#[derive(PartialEq, Clone, Copy)]
struct CloudKey {
    seed: u32,
    w: u32,
    /// `(octaves, warp inner, storm_cells, morph phases)` — `None` when the
    /// planet has no deck.
    deck: Option<(u32, u32, u32, u8)>,
    /// The baked base surface: `(lod thinning, shape hash, plane count)`.
    /// `None` when this planet's base is not baked — either its family cannot
    /// be, or the caller did not ask for it.
    base: Option<(u32, u64, u8)>,
}

impl CloudKey {
    /// Heap the baked map will occupy — one `u8` plane per scalar field it
    /// holds, three for the interleaved albedo.
    fn bytes(&self) -> usize {
        let texels = (self.w * (self.w / 2)) as usize;
        texels * (2 * self.deck.map_or(0, |d| d.3 as usize) + self.base.map_or(0, |b| b.2 as usize))
    }
}

/// How many baked layers to keep. A scene draws every planet in the system on
/// the way to drawing one, so a one-deep cache would evict on every body and
/// re-bake on the next — turning the optimization into a pessimization. Eight
/// covers `solar`'s largest roster.
const CLOUD_CACHE_SLOTS: usize = 8;

/// ...but only up to a budget, because the slots are not the same size. Zoomed
/// in, one map is 1 MB; eight of those is a heap growth in wasm for maps that
/// are mostly off-screen. Whichever limit binds first wins.
const CLOUD_CACHE_BYTES: usize = 24 << 20;

thread_local! {
    /// Per-thread, most-recently-used first. Native rendering fans frames across
    /// rayon, so each worker bakes its own copies once and then reuses them for
    /// every frame it is handed; wasm is single-threaded and bakes exactly once
    /// per planet per zoom level.
    static CLOUD_CACHE: RefCell<Vec<(CloudKey, Rc<CloudMap>)>> = const { RefCell::new(Vec::new()) };
}

/// Map width for a disc of `rad` pixels, as a power of two.
///
/// The visible hemisphere is half the map, spread over `2·rad` pixels, so
/// `w = 4·rad` puts one texel on one pixel. Rounding up to a power of two is
/// what keeps solar's adaptive detail cap from re-baking on every nudge — the
/// radius has to cross an octave before the key changes.
fn cloud_map_w(rad: f32) -> u32 {
    let want = (4.0 * rad).max(1.0);
    let pow2 = 1u32 << (32 - (want as u32).leading_zeros()).min(31);
    pow2.clamp(128, 1024)
}

/// The frozen layer for this planet, baked on first use and kept until the key
/// moves. Returns `None` when the planet has no weather to freeze.
fn cloud_map(ct: &PType, seed: u32, ofs: [f32; 3], lod: Lod, feat: u32, rad: f32) -> Option<Rc<CloudMap>> {
    let deck = (feat & F_BAKED_CLOUDS != 0 && ct.clouds > 0.0).then(|| {
        let o = lod.cld(4);
        let phases = if feat & F_MORPH_LUT != 0 { MORPH_PHASES } else { 1 };
        (o, warp_inner(feat, o), ct.storm_cells.to_bits(), phases)
    });
    // Which switch owns a family's base, and how many planes it needs.
    let planes = match ct.base {
        Base::Terrestrial | Base::Cratered if feat & F_BAKED_SURFACE != 0 => 3,
        Base::Emissive if feat & F_BAKED_SURFACE != 0 => 1,
        Base::Cloudy if feat & F_BAKED_CLOUDS != 0 => 1,
        Base::Banded if feat & F_BAKED_BANDS != 0 => 2,
        _ => 0,
    };
    let base = (planes > 0).then(|| (lod.surface, base_shape_key(ct, feat), planes));
    if deck.is_none() && base.is_none() {
        return None; // nothing to freeze
    }
    let key = CloudKey { seed, w: cloud_map_w(rad), deck, base };
    CLOUD_CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        if let Some(i) = cache.iter().position(|(k, _)| *k == key) {
            let hit = cache.remove(i);
            let m = Rc::clone(&hit.1);
            cache.insert(0, hit); // most-recently-used first
            return Some(m);
        }
        // Evict before baking: at 1024x512 a map is 1 MB, and holding the old
        // one while building the new one straddles a wasm heap growth.
        let want = key.bytes();
        while !cache.is_empty()
            && (cache.len() >= CLOUD_CACHE_SLOTS
                || cache.iter().map(|(k, _)| k.bytes()).sum::<usize>() + want > CLOUD_CACHE_BYTES)
        {
            cache.pop();
        }
        let m = Rc::new(bake_cloud_map(ct, seed, ofs, lod, feat, &key));
        cache.insert(0, (key, Rc::clone(&m)));
        Some(m)
    })
}

fn bake_cloud_map(ct: &PType, seed: u32, ofs: [f32; 3], lod: Lod, feat: u32, key: &CloudKey) -> CloudMap {
    let w = key.w;
    let h = w / 2;
    let n = (w * h) as usize;
    let sized = |on: bool| if on { vec![0u8; n] } else { Vec::new() };
    let phases = key.deck.map_or(1, |d| d.3);
    let deck_n = n * phases as usize;
    let (mut warp, mut dens) = if key.deck.is_some() {
        (vec![0u8; deck_n], vec![0u8; deck_n])
    } else {
        (Vec::new(), Vec::new())
    };
    let planes = key.base.map_or(0, |b| b.2);
    let mut base_a = sized(planes == 1 || planes == 2);
    let mut base_b = sized(planes == 2);
    let mut surf = if planes == 3 { vec![0u8; n * 3] } else { Vec::new() };
    // Vortex centers, hoisted: they are per-seed constants, and the live path
    // pays for them per pixel.
    let mut vort = [(0.0f32, 0.0f32); 2];
    for k in 0..2 {
        vort[k] = (
            (hash3(seed as i32, k as i32 * 7 + 1, 3) * 2.0 - 1.0) * 1.6 + ofs[0],
            (hash3(seed as i32, k as i32 * 7 + 2, 3) * 2.0 - 1.0) * 1.6 + ofs[2],
        );
    }
    for j in 0..h {
        let y = -1.0 + 2.0 * (j as f32 + 0.5) / h as f32;
        let r = (1.0 - y * y).max(0.0).sqrt();
        for i in 0..w {
            let lon = TAU * (i as f32 + 0.5) / w as f32;
            let (sl, cl) = lon.sin_cos();
            let (sx, sz) = (r * cl, r * sl);
            let t = (j * w + i) as usize;

            if let Some((oct, inner, _, _)) = key.deck {
                let (mut cx3, mut cz3) = (sx + ofs[0], sz + ofs[2]);
                if ct.storm_cells > 0.0 {
                    for (vx, vz) in vort {
                        let (dx, dz) = (cx3 - vx, cz3 - vz);
                        let fall = (-(dx * dx + dz * dz) * 2.2).exp();
                        let (ss, sc) = (fall * STORM_STATIC * 1.6 * ct.storm_cells).sin_cos();
                        cx3 = vx + dx * sc - dz * ss;
                        cz3 = vz + dx * ss + dz * sc;
                    }
                }
                for k in 0..phases as usize {
                    // Phases are laid out across the morph's RANGE, not across
                    // time: it oscillates, so the table is indexed by the value
                    // and walked back and forth. One phase means morph 0, the
                    // point the live cycle passes through twice a turn.
                    let morph = morph_of_phase(k, phases);
                    let py = y * 2.8 + ofs[1] + morph;
                    let (zx, zz) = (cx3 * 2.8, cz3 * 2.8 + morph);
                    let o = k * n + t;
                    warp[o] = q8(fbm_warp_inner(zx, py, zz, oct, inner, 0.9));
                    dens[o] = q8(fbm(zx, py, zz, oct));
                }
            }

            if key.base.is_some() {
                let (px, py, pz) = (sx + ofs[0], y + ofs[1], sz + ofs[2]);
                match ct.base {
                    Base::Terrestrial | Base::Cratered => {
                        let col = static_albedo(ct, y, px, py, pz, lod);
                        surf[t * 3] = q8(col[0]);
                        surf[t * 3 + 1] = q8(col[1]);
                        surf[t * 3 + 2] = q8(col[2]);
                    }
                    Base::Cloudy => {
                        // The whole of the mix factor, not just its noise: `band`
                        // folds in only `y` and the field, both known here, so the
                        // per-pixel cost collapses to one `mix`.
                        let o = lod.surf(5);
                        let flow = (0.5 + 0.3 * (y * 3.0).cos()) * SHEAR_STATIC;
                        let tv = fbm_warp_inner((px + flow) * 2.0, py * 2.0, pz * 2.0, o, warp_inner(feat, o), 0.7);
                        let band = 0.5 + 0.5 * (y * ct.bands + (tv - 0.5) * 6.0 * ct.turb).sin();
                        base_a[t] = q8(band * 0.6 + tv * 0.4);
                    }
                    Base::Emissive => {
                        // Only the rock field. The flow that lights it advects in
                        // three dimensions and cannot be a lookup offset, so it
                        // stays live — which is why a lava world still flows.
                        base_a[t] = q8(contrast(fbm(px * ct.freq, py * ct.freq, pz * ct.freq, lod.surf(6)), 1.7));
                    }
                    Base::Banded => {
                        // Baked at zero drift; the live path puts the drift back
                        // as a longitude offset on the lookup.
                        let o = lod.surf(5);
                        let warp = fbm_warp_inner(px * 1.3, py * 1.3, pz * 1.3, o, warp_inner(feat, o), 0.8);
                        let lat = y + (warp - 0.5) * ct.turb;
                        base_a[t] = q8(0.5 + 0.5 * (lat * ct.bands).sin());
                        base_b[t] = q8(smoothstep(0.55, 0.8, fbm(px * 4.0, py * 4.0, pz * 4.0, 4)));
                    }
                }
            }
        }
    }
    CloudMap { w, h, warp, dens, phases, base_a, base_b, surf }
}

/// The morph value phase `k` of `phases` is baked at, spanning the full cycle.
/// A single phase sits at 0 — the value the live cycle crosses twice a turn.
#[inline(always)]
fn morph_of_phase(k: usize, phases: u8) -> f32 {
    if phases <= 1 {
        0.0
    } else {
        MORPH_SPAN * (2.0 * k as f32 / (phases - 1) as f32 - 1.0)
    }
}

#[inline(always)]
fn q8(v: f32) -> u8 {
    (clamp01(v) * 255.0 + 0.5) as u8
}

/// Longitude of `(x, z)` as a **turn** in `[-0.5, 0.5)` — `atan2(z, x) / τ`.
///
/// The one place this is used is a bilinear index into [`CloudMap`], so what
/// matters is the error in texels, not in radians. The classic minimax cubic in
/// `a²` peaks at 2.0e-4 rad (measured, at the 45° fold where the two branches
/// meet and the error is a small step rather than a wobble). At the widest map
/// this builds, 1024 texels to the turn, that is 0.034 of a texel — a fortieth
/// of the filter's own smoothing, and far under the `u8` a texel is stored in.
/// libm's correctly-rounded `atan2f` is the wrong tool for that, and it was 6%
/// of a baked frame.
///
/// Plain `f32` arithmetic with no FMA, so wasm and native agree bit-for-bit —
/// the same rule `noise-core`'s `lanes.rs` documents.
#[inline(always)]
fn atan2_turns(z: f32, x: f32) -> f32 {
    const C: [f32; 3] = [-0.046_496_474, 0.159_314_22, -0.327_622_76];
    let (ax, az) = (x.abs(), z.abs());
    // Ratio of the smaller to the larger keeps the polynomial on [0, 1]. The
    // max() is the origin guard: both zero yields 0, an arbitrary but finite
    // longitude for a point that has none.
    let (num, den, folded) = if ax >= az { (az, ax, false) } else { (ax, az, true) };
    let a = num / den.max(f32::MIN_POSITIVE);
    let s = a * a;
    let mut r = ((C[0] * s + C[1]) * s + C[2]) * s * a + a; // atan(a), a in [0,1]
    if folded {
        r = FRAC_PI_2 - r;
    }
    if x < 0.0 {
        r = PI - r;
    }
    if z < 0.0 {
        r = -r;
    }
    r * (1.0 / TAU)
}

impl CloudMap {
    /// Texel addresses and weights for a bilinear fetch: `u` is a turn about the
    /// axis (any real — it wraps), `v` is `y` remapped to 0..1 and clamps, since
    /// there is nothing past a pole. Shared so the scalar and RGB fetches cannot
    /// disagree about where a sample lands.
    #[inline(always)]
    fn addr(&self, u: f32, v: f32) -> (usize, usize, usize, usize, f32, f32) {
        let fx = (u - u.floor()) * self.w as f32 - 0.5; // wrap first: fx > -1
        let fy = (v * self.h as f32 - 0.5).clamp(0.0, self.h as f32 - 1.0);
        let (x0, y0) = (fx.floor(), fy.floor());
        let xa = if x0 < 0.0 { self.w - 1 } else { x0 as u32 };
        let xb = if xa + 1 >= self.w { 0 } else { xa + 1 };
        let ya = y0 as u32;
        let yb = (ya + 1).min(self.h - 1);
        ((ya * self.w) as usize, (yb * self.w) as usize, xa as usize, xb as usize, fx - x0, fy - y0)
    }

    /// Where in the phase table a morph value falls: the lower plane and the
    /// blend to the next. Clamped, so the ends of the cycle hold rather than
    /// wrap into each other.
    #[inline(always)]
    fn phase_at(&self, morph: f32) -> (usize, f32) {
        if self.phases <= 1 {
            return (0, 0.0);
        }
        let last = (self.phases - 1) as f32;
        let t = ((morph / MORPH_SPAN + 1.0) * 0.5 * last).clamp(0.0, last);
        let k = (t.floor() as usize).min(self.phases as usize - 2);
        (k, t - k as f32)
    }

    /// Bilinear fetch from plane `k`, blended into plane `k + 1`.
    ///
    /// The two planes are independent noise at the fine octaves, so this is a
    /// dissolve rather than a slide — which is what the morph was for: weather
    /// that forms and dissipates instead of sliding rigidly.
    #[inline(always)]
    fn sample_phase(&self, tab: &[u8], k: usize, frac: f32, u: f32, v: f32) -> f32 {
        let n = (self.w * self.h) as usize;
        let a = self.sample(&tab[k * n..], u, v);
        if frac <= 0.0 {
            return a;
        }
        lerp(a, self.sample(&tab[(k + 1) * n..], u, v), frac)
    }

    /// Bilinear fetch of the interleaved albedo plane.
    ///
    /// Bilinear despite `ramp` being a hard step function, which makes every
    /// coastline a colour discontinuity: at the width this bakes (~one texel per
    /// pixel) the filter is close to identity, while nearest makes texels pop as
    /// the planet turns. Past the 1024 cap it does soften those edges — that is
    /// the visible cost, and it lands where you are most zoomed in.
    #[inline(always)]
    fn sample_rgb(&self, u: f32, v: f32) -> Rgb {
        let (ra, rb, xa, xb, tx, ty) = self.addr(u, v);
        let (ra, rb, xa, xb) = (ra * 3, rb * 3, xa * 3, xb * 3);
        let mut out = [0.0f32; 3];
        for (c, o) in out.iter_mut().enumerate() {
            let top = lerp(self.surf[ra + xa + c] as f32, self.surf[ra + xb + c] as f32, tx);
            let bot = lerp(self.surf[rb + xa + c] as f32, self.surf[rb + xb + c] as f32, tx);
            *o = lerp(top, bot, ty) * (1.0 / 255.0);
        }
        out
    }

    /// Bilinear fetch. `u` is a turn about the axis (any real — it wraps), `v`
    /// is `y` remapped to 0..1 and clamps, since there is nothing past a pole.
    #[inline(always)]
    fn sample(&self, tab: &[u8], u: f32, v: f32) -> f32 {
        let (ra, rb, xa, xb, tx, ty) = self.addr(u, v);
        let top = lerp(tab[ra + xa] as f32, tab[ra + xb] as f32, tx);
        let bot = lerp(tab[rb + xa] as f32, tab[rb + xb] as f32, tx);
        lerp(top, bot, ty) * (1.0 / 255.0)
    }
}

#[allow(clippy::too_many_arguments)]
/// Base albedo for `Terrestrial` and `Cratered`: a pure function of a direction
/// on the sphere, with no `angle` term anywhere. That is exactly the property
/// [`F_BAKED_SURFACE`] exploits, and the reason these two families can be baked
/// while the banded and emissive ones cannot.
///
/// `sy` is the sphere point's y (the ice caps are a latitude band); `px/py/pz`
/// are the same point with the seed offset already added, which is the domain
/// the noise is sampled in.
fn static_albedo(ct: &PType, sy: f32, px: f32, py: f32, pz: f32, lod: Lod) -> Rgb {
    match ct.base {
        Base::Cratered => {
            let m = smoothstep(0.4, 0.6, fbm(px * 1.2, py * 1.2, pz * 1.2, lod.surf(5)));
            let base_col = mix(ct.dark, ct.light, m);
            let w = worley(px * ct.freq, py * ct.freq, pz * ct.freq);
            let bowl = smoothstep(0.0, 0.35, w);
            let rim = smoothstep(0.30, 0.42, w) * (1.0 - smoothstep(0.42, 0.60, w));
            [
                clamp01(base_col[0] * (0.55 + 0.45 * bowl) + rim * 0.30),
                clamp01(base_col[1] * (0.55 + 0.45 * bowl) + rim * 0.30),
                clamp01(base_col[2] * (0.55 + 0.45 * bowl) + rim * 0.30),
            ]
        }
        // Terrestrial, and the fallback for anything that asks by mistake.
        _ => {
            let raw = fbm(px * ct.freq, py * ct.freq, pz * ct.freq, lod.surf(if ct.ridged { 5 } else { 6 }));
            let n = if ct.ridged { 1.0 - (2.0 * raw - 1.0).abs() } else { raw };
            let h = contrast(n, ct.contrast);
            let col = ramp(ct.stops, h);
            let cap = smoothstep(0.72, 0.9, sy.abs()) * ct.caps;
            mix(col, [0.92, 0.95, 1.0], cap)
        }
    }
}

/// Everything a baked base plane depends on, folded into one value the cache can
/// compare. Over-inclusive on purpose: a field that no family reads costs one
/// hash step, while a field left out silently serves a stale map.
///
/// `stops` is keyed by pointer: it is `&'static`, so its address identifies the
/// palette without walking it. The rest are `f32` bit patterns, since `f32` is
/// not `Eq` and a slider can move any of them.
fn base_shape_key(ct: &PType, feat: u32) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix1 = |v: u64| {
        h ^= v;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    mix1(ct.base as u64);
    mix1(ct.freq.to_bits() as u64);
    mix1(ct.contrast.to_bits() as u64);
    mix1(ct.ridged as u64);
    mix1(ct.stops.as_ptr() as usize as u64);
    mix1(ct.stops.len() as u64);
    mix1(ct.caps.to_bits() as u64);
    mix1(ct.bands.to_bits() as u64);
    mix1(ct.turb.to_bits() as u64);
    // The warp's inner-octave count is a render mode, not a type field.
    mix1(warp_inner(feat, 5) as u64);
    for c in ct.dark.iter().chain(ct.light.iter()) {
        mix1(c.to_bits() as u64);
    }
    h
}

fn surface(
    ct: &PType,
    sx: f32,
    sy: f32,
    sz: f32,
    ofs: [f32; 3],
    angle: f32,
    lod: Lod,
    feat: u32,
    baked: Option<&CloudMap>,
) -> (Rgb, f32) {
    let (px, py, pz) = (sx + ofs[0], sy + ofs[1], sz + ofs[2]);
    let (mut col, mut emis) = match ct.base {
        // The two families that hold still. Everything below the `match` — the
        // aurora, the lightning, the shading — still runs live; only the albedo
        // comes out of the table.
        Base::Terrestrial | Base::Cratered => {
            let col = match baked.filter(|m| !m.surf.is_empty()) {
                Some(m) => m.sample_rgb(atan2_turns(sz, sx), (sy + 1.0) * 0.5),
                None => static_albedo(ct, sy, px, py, pz, lod),
            };
            (col, 0.0f32)
        }
        Base::Banded => {
            // Zonal jets: adjacent latitude bands drift in opposite directions,
            // continuously — the real gas-giant look, not a uniform wobble. The
            // rate is per-latitude and changes sign, which is what makes
            // neighbouring bands shear past each other.
            let flow = angle * 0.16 * (sy * ct.bands * 0.5).sin();
            let (band, fine) = match baked.filter(|m| !m.base_b.is_empty()) {
                Some(m) => {
                    // Same rate, applied as a rotation in longitude rather than
                    // a shift of the noise domain — one subtraction from the
                    // texture coordinate and the bands turn for free. The unit
                    // is a turn, and near the equator an arc of `flow` radians
                    // on the unit sphere is the same distance the shear moved.
                    let (u, v) = (atan2_turns(sz, sx), (sy + 1.0) * 0.5);
                    let k = flow * (1.0 / TAU);
                    // Two planes because the two fields drift at different rates.
                    (m.sample(&m.base_a, u - k, v), m.sample(&m.base_b, u - k * 1.4, v))
                }
                None => {
                    // Domain warp makes the band turbulence curl and marble like fluid.
                    let o = lod.surf(5);
                    let warp = fbm_warp_inner((px + flow) * 1.3, py * 1.3, pz * 1.3, o, warp_inner(feat, o), 0.8);
                    let lat = sy + (warp - 0.5) * ct.turb;
                    let fine = fbm((px + flow * 1.4) * 4.0, py * 4.0, pz * 4.0, 4);
                    (0.5 + 0.5 * (lat * ct.bands).sin(), smoothstep(0.55, 0.8, fine))
                }
            };
            let mut col = mix(mix(ct.dark, ct.light, band), ct.light, fine * 0.35);
            if ct.spot > 0.0 {
                col = great_spot(col, sx, sy, sz, angle, ct.spot);
            }
            (col, 0.0)
        }
        Base::Emissive => {
            // The rock field holds still, so it bakes; the flow that lights it
            // advects in three dimensions — the field *evolving*, not moving —
            // and no lookup offset represents that, so it stays live at full
            // rate. 6 of the 9 octaves go, and the glow still flows.
            let n = match baked.filter(|m| !m.base_a.is_empty()) {
                Some(m) => m.sample(&m.base_a, atan2_turns(sz, sx), (sy + 1.0) * 0.5),
                None => contrast(fbm(px * ct.freq, py * ct.freq, pz * ct.freq, lod.surf(6)), 1.7),
            };
            // Molten flow: a slow noise field advects across the surface, so the
            // glow brightens and dims in drifting patches instead of pulsing.
            let flow = fbm(px * 2.2 + angle * 0.7, py * 2.2, pz * 2.2 - angle * 0.5, 3);
            let glow = clamp01(smoothstep(ct.glow_e0, ct.glow_e1, n) * (0.55 + 0.9 * flow));
            // Palette cycling: warm colors flow along the glow over time.
            let mid = mix(ct.glow_lo, ct.glow_hi, 0.5);
            let gcol = cycle3(ct.glow_lo, mid, ct.glow_hi, n * 1.6 + angle * 0.12);
            (mix(ct.rock, gcol, glow), glow)
        }
        Base::Cloudy => {
            // Storm bands churn: latitude-dependent shear + domain warp for
            // roiling, fluid-looking cloud cover. On a shrouded world this IS
            // the weather, so it is what F_BAKED_CLOUDS freezes here — and the
            // baked plane already holds the finished mix factor, so the frozen
            // path is a table read and nothing else.
            let f = match baked.filter(|m| !m.base_a.is_empty()) {
                Some(m) => m.sample(&m.base_a, atan2_turns(sz, sx), (sy + 1.0) * 0.5),
                None => {
                    let flow = (0.5 + 0.3 * (sy * 3.0).cos()) * angle.sin();
                    let o = lod.surf(5);
                    let t = fbm_warp_inner((px + flow) * 2.0, py * 2.0, pz * 2.0, o, warp_inner(feat, o), 0.7);
                    let band = 0.5 + 0.5 * (sy * ct.bands + (t - 0.5) * 6.0 * ct.turb).sin();
                    clamp01(band * 0.6 + t * 0.4)
                }
            };
            (mix(ct.dark, ct.light, f), 0.0)
        }
    };

    // Aurora — shimmering polar curtains, hue palette-cycled over time/latitude
    // (green → cyan → violet). Glows on the night side via emis.
    if ct.aurora > 0.0 {
        let a = aurora_glow(sx, sy, sz, angle) * ct.aurora;
        let ac = cycle3([0.25, 0.95, 0.45], [0.35, 0.85, 0.95], [0.65, 0.40, 1.0], sy * 1.4 + angle * 0.1);
        col[0] = clamp01(col[0] + ac[0] * a);
        col[1] = clamp01(col[1] + ac[1] * a);
        col[2] = clamp01(col[2] + ac[2] * a);
        emis = emis.max(a * 0.85);
    }
    // Lightning — small randomized-color flashes in storm cover.
    if ct.lightning > 0.0 {
        let (mag, lc) = lightning_flash(sx, sy, angle);
        let f = mag * ct.lightning;
        col[0] = clamp01(col[0] + lc[0] * f);
        col[1] = clamp01(col[1] + lc[1] * f);
        col[2] = clamp01(col[2] + lc[2] * f);
        emis = emis.max(f);
    }
    (col, emis)
}

fn star_bg(ix: u32, iy: u32, seed: u32) -> [u8; 4] {
    let h = hash3(ix as i32, iy as i32, seed as i32);
    if h > 0.986 {
        let b = (150.0 + 105.0 * (h - 0.986) / 0.014) as u8;
        [b, b, b, 255]
    } else {
        [9, 8, 20, 255]
    }
}

/// Number of tunable parameters exposed to the web sliders (see `param`).
pub const NUM_PARAMS: usize = 13;

/// A tunable parameter of a type, by index (must match `render_rgba_params`):
/// 0 contrast, 1 frequency, 2 specular, 3 shininess, 4 clouds, 5 caps,
/// 6 spot, 7 lightning, 8 aurora, 9 storm_cells, 10 bands, 11 turbulence,
/// 12 spec_albedo (specular follows surface brightness).
pub fn param(type_idx: usize, which: u32) -> f32 {
    let ct = &TYPES[type_idx % TYPES.len()];
    match which {
        0 => ct.contrast,
        1 => ct.freq,
        2 => ct.specular,
        3 => ct.shininess,
        4 => ct.clouds,
        5 => ct.caps,
        6 => ct.spot,
        7 => ct.lightning,
        8 => ct.aurora,
        9 => ct.storm_cells,
        10 => ct.bands,
        11 => ct.turb,
        12 => ct.spec_albedo,
        _ => 0.0,
    }
}

/// Render one planet frame as RGBA into `out` (must be >= size*size*4 bytes).
/// `angle` is the rotation in radians; a full 2π loop is seamless.
pub fn render_rgba(size: u32, type_idx: usize, seed: u32, angle: f32, out: &mut [u8]) {
    render_ct(size, &TYPES[type_idx % TYPES.len()], seed, angle, &Style::natural(), out);
}

/// Same as [`render_rgba`] but with a few parameters overridden (web sliders).
#[allow(clippy::too_many_arguments)]
pub fn render_rgba_custom(
    size: u32,
    type_idx: usize,
    seed: u32,
    angle: f32,
    contrast: f32,
    freq: f32,
    specular: f32,
    shininess: f32,
    out: &mut [u8],
) {
    let mut ct = TYPES[type_idx % TYPES.len()];
    ct.contrast = contrast;
    ct.freq = freq;
    ct.specular = specular;
    ct.shininess = shininess;
    render_ct(size, &ct, seed, angle, &Style::natural(), out);
}

/// Render with a full parameter override array (`NUM_PARAMS` values, same order
/// as [`param`]). Used by the web sliders.
pub fn render_rgba_params(size: u32, type_idx: usize, seed: u32, angle: f32, p: &[f32], out: &mut [u8]) {
    render_rgba_styled(size, type_idx, seed, angle, p, 0, 0.7, 1, out);
}

/// Like `render_rgba_params` but with global style: `palette` (0 natural, 1 game
/// boy, 2 ice, 3 sunset), `dither` (0..1), and `moons` (0/1).
#[allow(clippy::too_many_arguments)]
pub fn render_rgba_styled(
    size: u32,
    type_idx: usize,
    seed: u32,
    angle: f32,
    p: &[f32],
    palette: u32,
    dither: f32,
    moons: u32,
    out: &mut [u8],
) {
    render_rgba_features(size, type_idx, seed, angle, p, palette, dither, moons, F_ALL, out)
}

/// [`render_rgba_styled`] with the feature switches exposed. `features` is a
/// mask of the `F_*` bits; pass [`F_ALL`] for the normal picture. This is what
/// the demo's ablation panel drives — switching one bit off and timing the
/// difference is how the per-feature costs in the README were measured.
#[allow(clippy::too_many_arguments)]
pub fn render_rgba_features(
    size: u32,
    type_idx: usize,
    seed: u32,
    angle: f32,
    p: &[f32],
    palette: u32,
    dither: f32,
    moons: u32,
    features: u32,
    out: &mut [u8],
) {
    let mut ct = TYPES[type_idx % TYPES.len()];
    if p.len() >= NUM_PARAMS {
        ct.contrast = p[0];
        ct.freq = p[1];
        ct.specular = p[2];
        ct.shininess = p[3];
        ct.clouds = p[4];
        ct.caps = p[5];
        ct.spot = p[6];
        ct.lightning = p[7];
        ct.aurora = p[8];
        ct.storm_cells = p[9];
        ct.bands = p[10];
        ct.turb = p[11];
        ct.spec_albedo = p[12];
    }
    let style = Style { palette, dither, moons: moons != 0, feat: features };
    render_ct(size, &ct, seed, angle, &style, out);
}

// ---------------------------------------------------------------------------
// Pixel-art output: global style, ordered dithering, limited palettes.
// ---------------------------------------------------------------------------

/// Global look settings (not per-type).
pub struct Style {
    pub palette: u32, // 0 natural, 1 game boy, 2 ice, 3 sunset
    pub dither: f32,  // 0..1 ordered-dither strength
    pub moons: bool,  // draw orbiting moons
    pub feat: u32,    // feature switches (see `F_*`); `F_ALL` is the normal value
}
impl Style {
    pub fn natural() -> Style {
        Style { palette: 0, dither: 0.7, moons: true, feat: F_ALL }
    }
}

// Curated brightness-ramp palettes (ordered dark -> light).
const PAL_GAMEBOY: &[Rgb] = &[[0.06, 0.22, 0.06], [0.19, 0.38, 0.19], [0.55, 0.67, 0.06], [0.61, 0.75, 0.06]];
const PAL_ICE: &[Rgb] =
    &[[0.04, 0.08, 0.18], [0.13, 0.26, 0.46], [0.35, 0.55, 0.78], [0.62, 0.80, 0.94], [0.92, 0.98, 1.0]];
const PAL_SUNSET: &[Rgb] =
    &[[0.10, 0.05, 0.20], [0.40, 0.12, 0.34], [0.78, 0.26, 0.34], [0.97, 0.55, 0.30], [1.0, 0.86, 0.56]];
fn palette(i: u32) -> Option<&'static [Rgb]> {
    match i {
        1 => Some(PAL_GAMEBOY),
        2 => Some(PAL_ICE),
        3 => Some(PAL_SUNSET),
        _ => None,
    }
}

/// Final per-pixel quantization: ordered dithering, or a limited palette ramp.
/// This is where the smooth terminator becomes a dithered one.
fn finalize(o: Rgb, bx: f32, style: &Style) -> Rgb {
    if let Some(pal) = palette(style.palette) {
        let lum = clamp01(o[0] * 0.3 + o[1] * 0.59 + o[2] * 0.11);
        let f = (lum + bx * 0.14) * (pal.len() as f32 - 1.0);
        let i = (f + 0.5).max(0.0).min(pal.len() as f32 - 1.0) as usize;
        pal[i]
    } else {
        quant(o, bx, 22.0, style.dither)
    }
}

// ---------------------------------------------------------------------------
// Framing: where the disc sits in the buffer, and what surrounds it
// ---------------------------------------------------------------------------

/// How one planet is laid out in the destination buffer. Everything below this
/// point — the surface shader, weather, rings, dithering — is shared; a [`Frame`]
/// is the *only* difference between the hero view and a scene sprite.
struct Frame {
    /// Destination edge length in px (always square).
    size: u32,
    /// Disc radius in px.
    rad: f32,
    /// Unit light direction in the frame's screen basis (+x right, +y up,
    /// +z toward the viewer).
    light: [f32; 3],
    /// Cut the planet out on transparent pixels — a tile a scene compositor can
    /// blit — instead of filling the frame with a starfield.
    sprite: bool,
    /// Octave budget. The hero framing is always [`LOD_FULL`].
    lod: Lod,
}

/// The hero framing's fixed key light, over the viewer's left shoulder.
fn key_light() -> [f32; 3] {
    let v = [-0.55, 0.45, 0.70f32];
    let m = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / m, v[1] / m, v[2] / m]
}

/// The hero framing: the planet fills a `size`×`size` starfield frame.
fn render_ct(size: u32, ct: &PType, seed: u32, angle: f32, style: &Style, out: &mut [u8]) {
    // 0.375 (was 0.42) leaves orbital margin for moons and rings.
    let rad = (size as f32 * 24.0 / 64.0) * ct.radius_scale;
    let frame = Frame { size, rad, light: key_light(), sprite: false, lod: LOD_FULL };
    render_frame(&frame, ct, seed, angle, style, out);
}

/// The style a scene sprite renders with: natural palette, the house dither, and
/// no orbiting moons — a scene composites its own bodies, and moons would inflate
/// every tile by a third to gain a one-pixel speck.
fn tile_style(feat: u32) -> Style {
    Style { palette: 0, dither: 0.7, moons: false, feat }
}

/// Render one planet as a **sprite tile**: the same shader as [`render_rgba`],
/// but cut out on transparency, sized to its disc, and lit from an arbitrary
/// direction so a scene can point the light at its star. `solar` blits these.
///
/// `rad_px` is the disc radius in pixels — the tile grows to fit it (plus a ring
/// margin), so a scene planet is always rendered at exactly the radius asked for.
/// `light` must be a unit vector in the tile's screen basis (+x right, +y up,
/// +z toward the viewer). `angle` turns the surface and advances the weather,
/// exactly as in the hero framing.
///
/// `lod_enabled` turns on octave thinning once the tile passes 200px — the same
/// switch `sun_core::render_star_tile` takes, and the same threshold. It only
/// bites when a body is zoomed in far enough to be expensive; below that the
/// tile is bit-identical either way. Pass `false` for reference output.
pub fn render_tile(type_idx: usize, seed: u32, angle: f32, light: [f32; 3], rad_px: f32, lod_enabled: bool) -> Tile {
    render_tile_features(type_idx, seed, angle, light, rad_px, lod_enabled, F_ALL)
}

/// [`render_tile`] with the feature switches exposed — the scene framing's
/// counterpart to [`render_rgba_features`]. A scene wants this for one bit in
/// particular: [`F_BAKED_CLOUDS`], which is not in [`F_ALL`].
#[allow(clippy::too_many_arguments)]
pub fn render_tile_features(
    type_idx: usize,
    seed: u32,
    angle: f32,
    light: [f32; 3],
    rad_px: f32,
    lod_enabled: bool,
    feat: u32,
) -> Tile {
    let ct = &TYPES[type_idx % TYPES.len()];
    // Rings reach `ring_outer` disc radii sideways; a plain world needs only a
    // pixel of slack for its dark limb. `radius_scale` — which shrinks a ringed
    // world so its rings fit the hero's fixed square — is deliberately ignored
    // here: the tile is grown instead of the planet shrunk.
    let margin = if ct.rings { rad_px * (ct.ring_outer - 1.0) + 1.5 } else { 1.5 };
    let size = (((rad_px + margin) * 2.0).ceil() as u32).max(6);
    let mut px = vec![0u8; (size * size * 4) as usize];
    let frame = Frame { size, rad: rad_px, light, sprite: true, lod: Lod::for_size(size, lod_enabled) };
    render_frame(&frame, ct, seed, angle, &tile_style(feat), &mut px);
    Tile { px, size }
}

fn render_frame(fr: &Frame, ct: &PType, seed: u32, angle: f32, style: &Style, out: &mut [u8]) {
    let size = fr.size;
    let lod = fr.lod;
    // Past the terminator `shade` bottoms out at the 0.10 ambient floor, and the
    // output then snaps to 22 levels — roughly 3 of which are reachable. The
    // fine octaves and the whole cloud deck cannot survive that, so they are not
    // computed there. Lightning fires at a seeded point anywhere on the disc and
    // lights the cloud deck when it does, so those types opt out wholesale;
    // aurora is confined to a polar band, so it opts out by latitude below
    // rather than excluding every type that merely has one.
    // Lightning fires at a seeded point anywhere on the disc and lights the
    // cloud deck when it does, so those types opt out wholesale. Aurora is
    // confined to a polar band, so it opts out by latitude instead — excluding
    // every type that merely *has* an aurora would rule out most of the table.
    let night_ok = style.feat & F_NIGHT_LOD != 0 && ct.lightning == 0.0 && ct.base != Base::Emissive;
    let (cx, cy) = (size as f32 / 2.0, size as f32 / 2.0);
    let ofs = seed_offsets(seed);
    let l = fr.light;
    let (sina, cosa) = angle.sin_cos();
    let has_atmo = ct.atmo != [0.0; 3];
    let rad = fr.rad;
    const RING_SQUASH: f32 = 0.38;

    // Bake the frozen deck once, outside the loop. `cld`/`warp_inner` are the
    // same octave counts the live path would have used, so the two agree on
    // detail and differ only in that this one does not move.
    // Any one of the three switches can want a map, and each owns a different
    // set of planes — gating the whole map on the cloud bit alone made the other
    // two silently free, which an ablation panel reports as "costs nothing".
    let cloud_bake = if style.feat & (F_BAKED_CLOUDS | F_BAKED_SURFACE | F_BAKED_BANDS) != 0 {
        cloud_map(ct, seed, ofs, lod, style.feat, rad)
    } else {
        None
    };

    // Precompute orbiting moons (mx, my, radius, depth, seed).
    let mut moons: [(f32, f32, f32, f32, f32); 2] = [(0.0, 0.0, 0.0, 0.0, 0.0); 2];
    let mut nmoon = 0usize;
    if style.moons {
        let count = (hash3(seed as i32, 50, 1) * 2.6) as usize; // 0..2
        for k in 0..count.min(2) {
            let ks = k as i32 * 5;
            // Orbit in the margin around the disc; moons cross in front / behind
            // at the top and bottom of the tilted orbit.
            let orbit = 1.16 + hash3(seed as i32, ks + 1, 2) * 0.14;
            let tilt = 0.34 + hash3(seed as i32, ks + 2, 2) * 0.30;
            let speed = 0.25 + hash3(seed as i32, ks + 3, 2) * 0.4;
            let phase = hash3(seed as i32, ks + 4, 2) * TAU;
            let mr = (0.12 + hash3(seed as i32, ks + 5, 2) * 0.09) * ct.radius_scale.max(0.6);
            let oa = angle * speed + phase;
            moons[k] = (oa.cos() * orbit, oa.sin() * orbit * tilt, mr, oa.sin(), k as f32 + 1.0);
            nmoon += 1;
        }
    }

    // An oversized tile is mostly empty: a ringed giant reserves out to
    // `ring_outer` disc radii sideways, so its tile is ~4.4r across for a 2r
    // disc — only ~16% of it is ever drawn. Bound each row to the content and
    // zero the rest, instead of running the ring/rim/moon tests and a quantize
    // on every transparent pixel.
    let row_span = |iy: u32| -> (u32, u32) {
        if !fr.sprite || style.moons {
            return (0, size);
        }
        let ny = (cy - (iy as f32 + 0.5)) / rad;
        let disc = 1.0 - ny * ny;
        let ring = if ct.rings {
            let t = ny / RING_SQUASH;
            ct.ring_outer * ct.ring_outer - t * t
        } else {
            f32::NEG_INFINITY
        };
        let half = disc.max(ring);
        if half < 0.0 {
            return (0, 0);
        }
        let h = half.sqrt() * rad;
        let lo = (cx - h - 1.0).floor().max(0.0) as u32;
        let hi = ((cx + h + 1.0).ceil().max(0.0) as u32).min(size);
        (lo.min(size), hi)
    };

    for iy in 0..size {
        let (rlo, rhi) = row_span(iy);
        for ix in rlo..rhi {
            let nx = (ix as f32 + 0.5 - cx) / rad;
            let ny = (cy - (iy as f32 + 0.5)) / rad;
            let d2 = nx * nx + ny * ny;

            let mut o;
            // Sprite coverage. The hero framing is opaque everywhere (the
            // starfield fills the corners), so this only ever matters for tiles.
            let mut a = 1.0f32;
            if d2 <= 1.0 {
                let nz = (1.0 - d2).sqrt();
                let sx = nx * cosa + nz * sina;
                let sy = ny;
                let sz = -nx * sina + nz * cosa;

                let diff = (nx * l[0] + ny * l[1] + nz * l[2]).max(0.0);
                // Past the terminator every colour is multiplied by ~0.10 and
                // then snapped to 22 levels, so the fine octaves and the cloud
                // layer cannot survive into the output. Drop them there.
                let night = night_ok && diff <= 0.0 && (ct.aurora == 0.0 || sy.abs() < 0.52);
                // The night path drops surface octaves, which the map was not
                // baked at — so past the terminator the shroud goes live. Both
                // `Cloudy` types have lightning and so never take that path;
                // this is belt and braces for a future row that does not.
                let (mut col, emis) = surface(
                    ct,
                    sx,
                    sy,
                    sz,
                    ofs,
                    angle,
                    if night { LOD_NIGHT } else { lod },
                    style.feat,
                    if night { None } else { cloud_bake.as_deref() },
                );
                if ct.clouds > 0.0 && !night {
                    // The deck rotates at 2x the surface either way — that is
                    // the parallax that makes weather read as a separate layer,
                    // and it loops. What the two paths disagree about is whether
                    // the field itself also evolves as it turns.
                    let (cs, cc) = (angle * 2.0).sin_cos();
                    let (cloud, sh) = if let Some(m) = cloud_bake.as_deref().filter(|m| !m.warp.is_empty()) {
                        // Frozen: one direction on the sphere, two table reads.
                        // `ofs` is folded into the bake, so what the map wants
                        // is the bare rotated sphere point.
                        let px = nx * cc + nz * cs;
                        let pz = -nx * cs + nz * cc;
                        let v = (ny + 1.0) * 0.5;
                        let (k, kf) = m.phase_at(angle.sin() * MORPH_SPAN);
                        let cloud = m.sample_phase(&m.warp, k, kf, atan2_turns(pz, px), v);
                        let sh = if style.feat & F_CLOUD_SHADOW == 0 {
                            1.0
                        } else {
                            // The live shadow reads the plain field 0.45 toward
                            // the light, which steps off the sphere. The map
                            // holds directions only, so the displaced point is
                            // read at its own longitude and the same y: the
                            // tangential half of the same offset, which is the
                            // half that moves the shadow across the deck.
                            let (qx, qz) = (px + l[0] * 0.45, pz + l[2] * 0.45);
                            let shadow = smoothstep(0.55, 0.72, m.sample_phase(&m.dens, k, kf, atan2_turns(qz, qx), v));
                            1.0 - 0.22 * shadow * ct.clouds
                        };
                        (cloud, sh)
                    } else {
                        // Live: the deck also slowly billows — a periodic morph
                        // reveals new structure so weather forms and dissipates
                        // rather than sliding rigidly.
                        let mut cx3 = nx * cc + nz * cs + ofs[0];
                        let mut cz3 = -nx * cs + nz * cc + ofs[2];
                        let morph = angle.sin() * 0.6;

                        // Rotating storm cells: swirl the cloud field around a couple
                        // of seeded vortex centers, spinning with the animation.
                        if ct.storm_cells > 0.0 {
                            for k in 0..2 {
                                let vx = (hash3(seed as i32, k * 7 + 1, 3) * 2.0 - 1.0) * 1.6 + ofs[0];
                                let vz = (hash3(seed as i32, k * 7 + 2, 3) * 2.0 - 1.0) * 1.6 + ofs[2];
                                let (dx, dz) = (cx3 - vx, cz3 - vz);
                                let fall = (-(dx * dx + dz * dz) * 2.2).exp();
                                // Bounded (periodic) swirl: the eddy churns back and forth
                                // rather than winding into ever-tighter rings as `angle`
                                // grows unbounded on the continuously-running web.
                                let sw = fall * (angle * 0.6).sin() * 1.6 * ct.storm_cells;
                                let (ss, sc) = sw.sin_cos();
                                cx3 = vx + dx * sc - dz * ss;
                                cz3 = vz + dx * ss + dz * sc;
                            }
                        }

                        let dens = |ox: f32, oz: f32| {
                            fbm((cx3 + ox) * 2.8, ny * 2.8 + ofs[1] + morph, (cz3 + oz) * 2.8 + morph, lod.cld(4))
                        };
                        // Wispy, fractal cloud tops (domain-warped) so they break into
                        // ragged fronts instead of clumping into round blobs. Shadow
                        // uses the cheap plain density.
                        let co = lod.cld(4);
                        let cloud = fbm_warp_inner(cx3 * 2.8, ny * 2.8 + ofs[1] + morph, cz3 * 2.8 + morph, co, warp_inner(style.feat, co), 0.9);

                        let sh = if style.feat & F_CLOUD_SHADOW == 0 { 1.0 } else {
                            let shadow = smoothstep(0.55, 0.72, dens(l[0] * 0.45, l[2] * 0.45));
                            1.0 - 0.22 * shadow * ct.clouds
                        };
                        (cloud, sh)
                    };
                    col = [col[0] * sh, col[1] * sh, col[2] * sh];

                    col = mix(col, [1.0, 1.0, 1.0], smoothstep(0.52, 0.70, cloud) * ct.clouds);
                }
                let shade = (0.10 + 0.90 * diff).max(emis);
                o = [col[0] * shade, col[1] * shade, col[2] * shade];
                if ct.specular > 0.0 {
                    let hm = ((l[0]).powi(2) + (l[1]).powi(2) + (l[2] + 1.0).powi(2)).sqrt();
                    let ndh = (nx * l[0] / hm + ny * l[1] / hm + nz * (l[2] + 1.0) / hm).max(0.0);
                    // Material-aware: darker surface reflects less specular. `col`
                    // is the un-shaded albedo; its luminance scales the glint so a
                    // moon's dark maria glare far less than its bright highlands.
                    let alb = col[0] * 0.3 + col[1] * 0.59 + col[2] * 0.11;
                    let mat = 1.0 - ct.spec_albedo * (1.0 - alb);
                    // Cycling shimmer so water/ice glints twinkle over time.
                    let shimmer = 0.82 + 0.18 * fbm(sx * 5.0 + angle * 2.5, sy * 5.0, sz * 5.0, 2);
                    let sp = ndh.powf(ct.shininess) * ct.specular * mat * shimmer;
                    o[0] = clamp01(o[0] + sp);
                    o[1] = clamp01(o[1] + sp);
                    o[2] = clamp01(o[2] + sp);
                }
                if has_atmo && style.feat & F_ATMO != 0 {
                    let rim = (1.0 - nz).powf(3.0) * 0.6;
                    o[0] = clamp01(o[0] + ct.atmo[0] * rim);
                    o[1] = clamp01(o[1] + ct.atmo[1] * rim);
                    o[2] = clamp01(o[2] + ct.atmo[2] * rim);
                }
            } else if fr.sprite {
                // Off the disc a tile is empty — the scene shows through.
                o = [0.0, 0.0, 0.0];
                a = 0.0;
            } else {
                let s = if style.feat & F_STARFIELD != 0 { star_bg(ix, iy, seed) } else { [9, 8, 20, 255] };
                o = [s[0] as f32 / 255.0, s[1] as f32 / 255.0, s[2] as f32 / 255.0];
            }

            if ct.rings {
                let rr = (nx * nx + (ny / RING_SQUASH).powi(2)).sqrt();
                if rr >= ct.ring_inner && rr <= ct.ring_outer && (ny < 0.0 || d2 > 1.0) {
                    let rn = (rr - ct.ring_inner) / (ct.ring_outer - ct.ring_inner);
                    let stripes = 0.5 + 0.5 * (rn * 36.0).sin();
                    let mut alpha = clamp01(0.30 + 0.55 * stripes);
                    if rn > 0.46 && rn < 0.54 {
                        alpha *= 0.12;
                    }
                    let rb = 0.55 + 0.45 * stripes;
                    let rc = [ct.ring_col[0] * rb, ct.ring_col[1] * rb, ct.ring_col[2] * rb];
                    if fr.sprite && d2 > 1.0 {
                        // The ring arc *past* the disc is the only translucent part
                        // of a tile. Hand the scene the ring's own colour with
                        // `alpha` as its coverage and let the blit do the blend —
                        // lerping toward the empty tile here instead would darken
                        // the ring by a second factor of `alpha` once composited.
                        o = rc;
                        a = alpha;
                    } else {
                        // Over the disc (or over the hero framing's starfield) the
                        // backdrop is already opaque, so blend in place.
                        o = [lerp(o[0], rc[0], alpha), lerp(o[1], rc[1], alpha), lerp(o[2], rc[2], alpha)];
                    }
                }
            }

            // Crisp dark rim on the planet disc. Applied BEFORE moons so a front
            // moon crossing the limb passes over the rim instead of being clipped
            // under it.
            if d2 <= 1.0 && style.feat & F_RIM != 0 {
                let edge = 1.0 - 1.3 / rad;
                if d2 > edge * edge {
                    o = [o[0] * 0.26, o[1] * 0.26, o[2] * 0.30];
                }
            }

            // Orbiting moons: front moons draw over everything (incl. the rim);
            // back moons only where the planet disc doesn't occlude them.
            for m in 0..nmoon {
                let (mx, my, mr, depth, ms) = moons[m];
                let ld2 = (nx - mx) * (nx - mx) + (ny - my) * (ny - my);
                if ld2 < mr * mr && (depth > 0.0 || d2 > 1.0) {
                    let (lnx, lny) = ((nx - mx) / mr, (ny - my) / mr);
                    let lnz = (1.0 - lnx * lnx - lny * lny).max(0.0).sqrt();
                    let mdiff = (lnx * l[0] + lny * l[1] + lnz * l[2]).max(0.0);
                    // The moon's own dark edge, for sprite consistency.
                    let mrim = 0.4 + 0.6 * smoothstep(0.0, 0.26, lnz);
                    let msh = (0.12 + 0.9 * mdiff) * mrim;
                    let t = fbm(lnx * 3.0 + ms * 9.0, lny * 3.0, ms * 5.0, 2);
                    let base = mix([0.30, 0.29, 0.33], [0.60, 0.59, 0.62], smoothstep(0.4, 0.6, t));
                    o = [base[0] * msh, base[1] * msh, base[2] * msh];
                    a = 1.0;
                }
            }

            let px = finalize(o, bayer(ix, iy), style);
            let idx = ((iy * size + ix) * 4) as usize;
            out[idx] = (clamp01(px[0]) * 255.0) as u8;
            out[idx + 1] = (clamp01(px[1]) * 255.0) as u8;
            out[idx + 2] = (clamp01(px[2]) * 255.0) as u8;
            out[idx + 3] = if fr.sprite { (clamp01(a) * 255.0) as u8 } else { 255 };
        }
    }
}

#[cfg(test)]
mod cloud_tests {
    use super::*;

    /// The fast longitude has to be good to well under a texel of the widest
    /// map (1024), or the frozen deck would shear along the seam where the
    /// approximation's fold sits.
    #[test]
    fn atan2_turns_tracks_libm() {
        let mut worst: f32 = 0.0;
        for i in 0..2000 {
            let a = (i as f32 / 2000.0) * TAU - PI;
            // Sweep radii too: the polynomial is fed a ratio, so a near-axis
            // point is a different case from a diagonal one.
            for &r in &[1.0f32, 0.05, 8.0] {
                let (x, z) = (r * a.cos(), r * a.sin());
                let want = z.atan2(x) * (1.0 / TAU);
                let got = atan2_turns(z, x);
                // Both wrap at ±0.5; compare the shorter way round.
                let d = (got - want).abs();
                worst = worst.max(d.min(1.0 - d));
            }
        }
        // 1024 texels to the turn, so a texel is 9.8e-4 of a turn. The cubic
        // lands at 3.3e-5 (2.0e-4 rad); the bound is where it stops being
        // negligible against the bilinear filter, not where it sits today.
        assert!(worst < 1.0e-4, "worst {worst} turns");
    }

    #[test]
    fn atan2_turns_handles_axes_and_origin() {
        for (z, x, want) in [(0.0, 1.0, 0.0), (1.0, 0.0, 0.25), (0.0, -1.0, 0.5), (-1.0, 0.0, -0.25)] {
            let got: f32 = atan2_turns(z, x);
            assert!((got - want).abs() < 1e-5, "atan2_turns({z}, {x}) = {got}, want {want}");
        }
        assert!(atan2_turns(0.0, 0.0).is_finite());
    }

    /// A bilinear fetch dead on a texel centre must return that texel, and the
    /// longitude axis must wrap rather than clamp — a clamped seam would show
    /// as a stationary band down the middle of every planet.
    #[test]
    fn cloud_map_sampling_is_exact_and_wraps() {
        let m = CloudMap { w: 4, h: 2, warp: vec![0, 64, 128, 255, 10, 20, 30, 40], dens: vec![], phases: 1, base_a: vec![], base_b: vec![], surf: vec![] };
        for i in 0..4 {
            let u = (i as f32 + 0.5) / 4.0;
            let got = m.sample(&m.warp, u, 0.25);
            assert!((got - m.warp[i] as f32 / 255.0).abs() < 1e-6, "texel {i}: {got}");
        }
        // Half a texel before column 0 blends columns 3 and 0, not 0 and 0.
        let seam = m.sample(&m.warp, 0.0, 0.25);
        let want = (255.0 + 0.0) / 2.0 / 255.0;
        assert!((seam - want).abs() < 1e-6, "seam {seam}, want {want}");
        // And a whole turn later is the same place.
        assert_eq!(m.sample(&m.warp, 0.0, 0.25), m.sample(&m.warp, 1.0, 0.25));
        assert_eq!(m.sample(&m.warp, 0.3, 0.25), m.sample(&m.warp, -0.7, 0.25));
    }

    /// The map is the whole sphere, so one bake serves every angle — and the
    /// cache must not rebuild for a change that cannot move it.
    #[test]
    fn cloud_map_cache_keys_on_what_it_depends_on() {
        let w = cloud_map_w(128.0);
        assert_eq!(w, cloud_map_w(200.0), "same octave of radius must reuse the bake");
        assert!(cloud_map_w(4096.0) <= 1024, "capped");
        assert!(cloud_map_w(1.0) >= 128, "floored");
    }
}
