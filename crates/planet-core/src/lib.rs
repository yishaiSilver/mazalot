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

use std::f32::consts::{PI, TAU};

pub use scene_core::Tile;

// ---------------------------------------------------------------------------
// Low-level primitives (noise, color/ramp helpers, ordered dither) now live in
// the shared `noise-core` / `dither-core` rlibs — byte-for-byte identical to the
// copies that used to live here. Imported below; `seed_offsets` keeps a thin
// local wrapper (span 256) so call sites and numeric output are unchanged.
// ---------------------------------------------------------------------------

use dither_core::{bayer, quant};
use noise_core::{
    clamp01, contrast, cycle3, fbm, fbm_warp, hash3, lerp, mix, ramp, smoothstep, worley, Rgb,
};

// ---------------------------------------------------------------------------
// Level of detail
// ---------------------------------------------------------------------------

/// Octave counts for the shader's fBm fields, derived from the disc's pixel
/// radius.
///
/// A tile puts one sphere radius across `rad` px, so a field sampled at
/// `p · freq` has its `k`-th octave's lattice cell land at
/// `rad / (freq · 2^(k-1))` px. Under two pixels that octave is past Nyquist:
/// unresolvable, and on a turning planet it reads as crawling speckle. Dropping
/// it is cheaper *and* steadier — a mip level, not a quality knob.
#[derive(Clone, Copy)]
struct Lod {
    /// Disc radius in px — how finely this tile can resolve anything.
    rad: f32,
    /// Hard ceiling on top of what the radius allows. `F_NIGHT_LOD` sets it
    /// past the terminator, where `shade` bottoms out at the 0.10 ambient floor
    /// and the 22-level output has ~3 levels left to say anything with.
    cap: u32,
}

impl Lod {
    fn for_disc(rad_px: f32) -> Lod {
        Lod { rad: rad_px.max(1.0), cap: u32::MAX }
    }

    /// The same disc with the octave count capped — see [`Lod::cap`].
    fn capped(self, cap: u32) -> Lod {
        Lod { cap, ..self }
    }

    /// Octaves for a domain warp's three displacement components.
    const WARP: u32 = 2;

    /// Solves `rad / (freq · 2^(k-1)) >= 2` for the largest whole `k`, clamped
    /// to `1..=full`.
    #[inline]
    fn oct(&self, freq: f32, full: u32) -> u32 {
        let cells = self.rad / (2.0 * freq.max(0.01)); // = 2^(k-1) at the limit
        if cells <= 1.0 {
            return 1;
        }
        let k = 1 + cells.log2() as u32;
        k.clamp(1, full.min(self.cap))
    }
}

// ---------------------------------------------------------------------------
// Feature switches
// ---------------------------------------------------------------------------
//
// The per-type sliders already reach most of the shader — `clouds`, `specular`,
// `spot`, `aurora`, `lightning`, `storm_cells` and `caps` are all gated on
// `> 0.0`, so zeroing one switches that feature off. These are the pieces a
// parameter cannot reach: parts of a layer rather than a whole one, framing
// furniture, and the optimizations. A SET bit means the feature is ON.

/// The cloud deck's self-shadow, independent of the cloud colour above it.
pub const F_CLOUD_SHADOW: u32 = 1;
/// The atmosphere rim glow at the limb.
pub const F_ATMO: u32 = 2;
/// The crisp 1px dark outline around the disc.
pub const F_RIM: u32 = 4;
/// The hashed starfield behind the planet (hero framing only).
pub const F_STARFIELD: u32 = 8;
/// OPTIMIZATION: cap the octaves and skip the cloud deck past the terminator,
/// where `shade` bottoms out at the 0.10 ambient floor and the 22-level output
/// has about three levels left to say anything with.
///
/// NOT in [`F_ALL`]. It used to be, back when the octave budget only reached the
/// base field — the night side quantized to the same levels either way. Now that
/// `Lod` also feeds the aurora and the great spot, capping it there moves
/// pixels, so it sits with the other switches that change the picture.
pub const F_NIGHT_LOD: u32 = 16;
/// OPTIMIZATION: run a domain warp's displacement fields at [`Lod::WARP`]
/// octaves instead of matching the field they bend.
pub const F_CHEAP_WARP: u32 = 32;
/// Everything on — what every caller but the demo's ablation panel wants. The
/// switches that change the picture rather than the pixel budget are outside it,
/// so `out/` stays byte-identical while the web demos opt in.
pub const F_ALL: u32 = F_CLOUD_SHADOW | F_ATMO | F_RIM | F_STARFIELD | F_CHEAP_WARP;

/// Octave ceiling past the terminator. Four keeps a terrestrial world's
/// coastlines; below that the night side loses shape, not just grain.
const NIGHT_OCT: u32 = 4;

/// Octaves for a domain warp's three displacement fields, given the count the
/// field they bend is running at. [`F_CHEAP_WARP`] is what the ablation panel
/// switches off to price the difference.
#[inline(always)]
fn warp_oct(feat: u32, main: u32) -> u32 {
    if feat & F_CHEAP_WARP != 0 {
        Lod::WARP
    } else {
        main
    }
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

/// The two seeded storm-cell centres, as `(x, z)` in the noise domain — `ofs` is
/// already folded in. One definition, because the CPU deck and the GL port both
/// have to swirl the clouds around the same two points.
fn vortex_centres(seed: u32, ofs: [f32; 3]) -> [(f32, f32); 2] {
    [0, 1].map(|k: i32| {
        (
            (hash3(seed as i32, k * 7 + 1, 3) * 2.0 - 1.0) * 1.6 + ofs[0],
            (hash3(seed as i32, k * 7 + 2, 3) * 2.0 - 1.0) * 1.6 + ofs[2],
        )
    })
}

/// The frame's orbiting moons as `(mx, my, radius, depth, moon seed)` in disc
/// radii, plus how many of the two slots are real (0..=2).
///
/// They orbit in the margin around the disc and cross in front of / behind it at
/// the top and bottom of a tilted orbit, which is what `depth`'s sign says.
fn moon_ring(ct: &PType, seed: u32, angle: f32) -> ([(f32, f32, f32, f32, f32); 2], usize) {
    let mut moons = [(0.0, 0.0, 0.0, 0.0, 0.0); 2];
    let count = (hash3(seed as i32, 50, 1) * 2.6) as usize; // 0..2
    for k in 0..count.min(2) {
        let ks = k as i32 * 5;
        let orbit = 1.16 + hash3(seed as i32, ks + 1, 2) * 0.14;
        let tilt = 0.34 + hash3(seed as i32, ks + 2, 2) * 0.30;
        let speed = 0.25 + hash3(seed as i32, ks + 3, 2) * 0.4;
        let phase = hash3(seed as i32, ks + 4, 2) * TAU;
        let mr = (0.12 + hash3(seed as i32, ks + 5, 2) * 0.09) * ct.radius_scale.max(0.6);
        let oa = angle * speed + phase;
        moons[k] = (oa.cos() * orbit, oa.sin() * orbit * tilt, mr, oa.sin(), k as f32 + 1.0);
    }
    (moons, count.min(2))
}

/// A drifting spiral cyclone (great-spot) tint on a banded world, with a calm eye.
fn great_spot(col: Rgb, sx: f32, sy: f32, sz: f32, angle: f32, intensity: f32, lod: Lod) -> Rgb {
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
    let base = ((dlon * 1.05).powi(2) + (dlat * 2.2).powi(2)).sqrt();
    // The boundary below scales `base` by `0.82 + 0.4·edge`, `edge` in [0, 1],
    // so 0.82·base is the smallest it can be: past that the pixel is outside
    // whatever the noise says. Most of a banded disc is. Exact rejection.
    if base * 0.82 >= 1.0 {
        return col;
    }
    // Turbulent, irregular boundary — not a clean geometric oval.
    let edge = fbm(dlon * 3.0 + sy * 4.0, dlat * 3.0, sz * 2.0, lod.oct(4.0, 2));
    let d = base * (0.82 + 0.4 * edge);
    if d >= 1.0 {
        return col;
    }
    // spiral streaks that churn with time; the streaks read as the vortex, no rim.
    let swirl = (1.0 - d) * 5.0 + angle * 1.2;
    let (s, c) = swirl.sin_cos();
    let lx = dlon * c - dlat * s;
    let ly = dlon * s + dlat * c;
    let streak = fbm(lx * 8.0, ly * 8.0, sy * 2.0, lod.oct(8.0, 4));
    let core = smoothstep(1.0, 0.15, d) * intensity;
    let spot_col = mix([0.80, 0.36, 0.26], [0.93, 0.66, 0.46], smoothstep(0.40, 0.82, streak));
    let mut out = mix(col, spot_col, core * 0.78);
    // Recognizable hurricane eye: a small calm dark center.
    let eye = smoothstep(0.20, 0.06, d) * intensity;
    out = mix(out, [0.28, 0.11, 0.10], eye * 0.7);
    out
}

/// Shimmering polar aurora intensity (0..1) at this surface point.
fn aurora_glow(sx: f32, sy: f32, sz: f32, angle: f32, lod: Lod) -> f32 {
    let lat = sy.abs();
    let band = smoothstep(0.55, 0.70, lat) * (1.0 - smoothstep(0.82, 0.96, lat));
    if band <= 0.0 {
        return 0.0;
    }
    let lon = sz.atan2(sx);
    // curtains: drift in longitude + shimmer over time
    let curtain = fbm(lon * 2.5 + angle * 1.5, lat * 9.0, sy * 3.0 + angle, lod.oct(9.0, 3));
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


/// Base albedo for `Terrestrial` and `Cratered`: a pure function of a direction
/// on the sphere, with no `angle` term anywhere — which is why these two share
/// one path while the banded, emissive and cloudy families each advect.
///
/// `sy` is the sphere point's y (the ice caps are a latitude band); `px/py/pz`
/// are the same point with the seed offset added, the domain the noise is in.
fn static_albedo(ct: &PType, sy: f32, px: f32, py: f32, pz: f32, lod: Lod) -> Rgb {
    match ct.base {
        Base::Cratered => {
            let m = smoothstep(0.4, 0.6, fbm(px * 1.2, py * 1.2, pz * 1.2, lod.oct(1.2, 5)));
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
            let raw = fbm(px * ct.freq, py * ct.freq, pz * ct.freq, lod.oct(ct.freq, if ct.ridged { 5 } else { 6 }));
            let n = if ct.ridged { 1.0 - (2.0 * raw - 1.0).abs() } else { raw };
            let h = contrast(n, ct.contrast);
            let col = ramp(ct.stops, h);
            let cap = smoothstep(0.72, 0.9, sy.abs()) * ct.caps;
            mix(col, [0.92, 0.95, 1.0], cap)
        }
    }
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
) -> (Rgb, f32) {
    let (px, py, pz) = (sx + ofs[0], sy + ofs[1], sz + ofs[2]);
    let (mut col, mut emis) = match ct.base {
        // The two families with no `angle` term at all — their albedo is a pure
        // function of a direction on the sphere.
        Base::Terrestrial | Base::Cratered => (static_albedo(ct, sy, px, py, pz, lod), 0.0f32),
        Base::Banded => {
            // Zonal jets: adjacent latitude bands drift in opposite directions,
            // continuously — the real gas-giant look, not a uniform wobble.
            let flow = angle * 0.16 * (sy * ct.bands * 0.5).sin();
            // Domain warp makes the band turbulence curl and marble like fluid.
            let o = lod.oct(1.3, 5);
            let warp = fbm_warp((px + flow) * 1.3, py * 1.3, pz * 1.3, warp_oct(feat, o), o, 0.8);
            let lat = sy + (warp - 0.5) * ct.turb;
            let band = 0.5 + 0.5 * (lat * ct.bands).sin();
            let fine = fbm((px + flow * 1.4) * 4.0, py * 4.0, pz * 4.0, lod.oct(4.0, 4));
            let mut col = mix(mix(ct.dark, ct.light, band), ct.light, smoothstep(0.55, 0.8, fine) * 0.35);
            if ct.spot > 0.0 {
                col = great_spot(col, sx, sy, sz, angle, ct.spot, lod);
            }
            (col, 0.0)
        }
        Base::Emissive => {
            let n = contrast(fbm(px * ct.freq, py * ct.freq, pz * ct.freq, lod.oct(ct.freq, 6)), 1.7);
            // Molten flow: a slow noise field advects across the surface, so the
            // glow brightens and dims in drifting patches instead of pulsing.
            let flow = fbm(px * 2.2 + angle * 0.7, py * 2.2, pz * 2.2 - angle * 0.5, lod.oct(2.2, 3));
            let glow = clamp01(smoothstep(ct.glow_e0, ct.glow_e1, n) * (0.55 + 0.9 * flow));
            // Palette cycling: warm colors flow along the glow over time.
            let mid = mix(ct.glow_lo, ct.glow_hi, 0.5);
            let gcol = cycle3(ct.glow_lo, mid, ct.glow_hi, n * 1.6 + angle * 0.12);
            (mix(ct.rock, gcol, glow), glow)
        }
        Base::Cloudy => {
            // Storm bands churn: latitude-dependent shear + domain warp for
            // roiling, fluid-looking cloud cover. On a shrouded world this IS
            // the surface, not a deck over one.
            let flow = (0.5 + 0.3 * (sy * 3.0).cos()) * angle.sin();
            let o = lod.oct(2.0, 5);
            let t = fbm_warp((px + flow) * 2.0, py * 2.0, pz * 2.0, warp_oct(feat, o), o, 0.7);
            let band = 0.5 + 0.5 * (sy * ct.bands + (t - 0.5) * 6.0 * ct.turb).sin();
            (mix(ct.dark, ct.light, clamp01(band * 0.6 + t * 0.4)), 0.0)
        }
    };

    // Aurora — shimmering polar curtains, hue palette-cycled over time/latitude
    // (green → cyan → violet). Glows on the night side via emis.
    if ct.aurora > 0.0 {
        let a = aurora_glow(sx, sy, sz, angle, lod) * ct.aurora;
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

/// A type row with the caller's parameter overrides applied. One copy, so the
/// whole-frame and banded entry points cannot disagree about what they render.
fn ct_with_params(type_idx: usize, p: &[f32]) -> PType {
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
    ct
}

/// [`render_rgba_features`] restricted to rows `y0..y1` of the frame.
///
/// The band is written into `out` at its real offset — `out` is a whole frame's
/// worth of pixels, and the rows outside the band are not touched. Splitting a
/// frame into N bands across N wasm instances and concatenating the results
/// reproduces the whole frame exactly.
#[allow(clippy::too_many_arguments)]
pub fn render_rgba_band(
    size: u32,
    type_idx: usize,
    seed: u32,
    angle: f32,
    p: &[f32],
    palette: u32,
    dither: f32,
    moons: u32,
    features: u32,
    y0: u32,
    y1: u32,
    out: &mut [u8],
) {
    let ct = ct_with_params(type_idx, p);
    let style = Style { palette, dither, moons: moons != 0, feat: features };
    render_ct_band(size, &ct, seed, angle, &style, y0, y1, out);
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
    let ct = ct_with_params(type_idx, p);
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
    /// The sub-rect to write, `[x0, y0, x1, y1)`; outside it is left alone. The
    /// hero framing passes the whole frame.
    clip: [u32; 4],
}

/// The hero framing's fixed key light, over the viewer's left shoulder.
fn key_light() -> [f32; 3] {
    let v = [-0.55, 0.45, 0.70f32];
    let m = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    [v[0] / m, v[1] / m, v[2] / m]
}

/// The hero framing: the planet fills a `size`×`size` starfield frame.
fn render_ct(size: u32, ct: &PType, seed: u32, angle: f32, style: &Style, out: &mut [u8]) {
    render_ct_band(size, ct, seed, angle, style, 0, size, out)
}

/// [`render_ct`] restricted to rows `y0..y1`, which is how one frame is split
/// across several wasm instances.
///
/// **Rows outside the band are left untouched**, so a caller hands each worker
/// its own buffer and stitches the bands back together. Every band is shaded
/// from the same pure function of pixel position, so the result is bit-identical
/// to rendering the frame whole — `render_band_matches_whole` pins that.
fn render_ct_band(size: u32, ct: &PType, seed: u32, angle: f32, style: &Style, y0: u32, y1: u32, out: &mut [u8]) {
    // 0.375 (was 0.42) leaves orbital margin for moons and rings.
    let rad = (size as f32 * 24.0 / 64.0) * ct.radius_scale;
    let clip = [0, y0.min(size), size, y1.min(size)];
    let frame = Frame { size, rad, light: key_light(), sprite: false, clip };
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
pub fn render_tile(type_idx: usize, seed: u32, angle: f32, light: [f32; 3], rad_px: f32) -> Tile {
    let size = tile_size(type_idx, rad_px);
    let mut tile = Tile::default();
    render_tile_into(&mut tile, type_idx, seed, angle, light, rad_px, [0, 0, size, size], F_ALL);
    tile
}

/// The edge length [`render_tile`] produces for this type at this radius —
/// needed *before* rendering, to ask `scene_core::visible_tile_rect` which part
/// of the tile will be seen.
pub fn tile_size(type_idx: usize, rad_px: f32) -> u32 {
    // Rings reach `ring_outer` disc radii sideways; a plain world needs only a
    // pixel of slack for its dark limb. `radius_scale` — which shrinks a ringed
    // world so its rings fit the hero's fixed square — is deliberately ignored
    // here: the tile is grown instead of the planet shrunk.
    let ct = &TYPES[type_idx % TYPES.len()];
    let margin = if ct.rings { rad_px * (ct.ring_outer - 1.0) + 1.5 } else { 1.5 };
    (((rad_px + margin) * 2.0).ceil() as u32).max(6)
}

/// [`render_tile`] into a tile you already own, shading only the tile pixels in
/// `clip` (`[x0, y0, x1, y1)`, tile px).
///
/// Pass `scene_core::visible_tile_rect` as the clip and the off-screen part of a
/// zoomed-in body's tile is never shaded.
///
/// **Pixels outside `clip` are left as they were**, not cleared: the tile is only
/// valid for the placement its clip came from. That is exactly what `blit` reads
/// back, so a reused buffer cannot leak a previous body into the scene — but do
/// not hand the tile to anything that reads wider.
#[allow(clippy::too_many_arguments)]
pub fn render_tile_into(
    tile: &mut Tile,
    type_idx: usize,
    seed: u32,
    angle: f32,
    light: [f32; 3],
    rad_px: f32,
    clip: [u32; 4],
    feat: u32,
) {
    let ct = &TYPES[type_idx % TYPES.len()];
    let size = tile_size(type_idx, rad_px);
    tile.ensure(size);
    let clip = [clip[0].min(size), clip[1].min(size), clip[2].min(size), clip[3].min(size)];
    let frame = Frame { size, rad: rad_px, light, sprite: true, clip };
    render_frame(&frame, ct, seed, angle, &tile_style(feat), &mut tile.px);
}

/// A glint below this cannot survive the 22-level quantization (one level is
/// 1/22 ≈ 0.045), so the shimmer noise modulating it is not worth evaluating.
const SPEC_FLOOR: f32 = 1.0 / 1024.0;

fn render_frame(fr: &Frame, ct: &PType, seed: u32, angle: f32, style: &Style, out: &mut [u8]) {
    let size = fr.size;
    let (cx, cy) = (size as f32 / 2.0, size as f32 / 2.0);
    let ofs = seed_offsets(seed);
    let l = fr.light;
    let (sina, cosa) = angle.sin_cos();
    let has_atmo = ct.atmo != [0.0; 3];
    let rad = fr.rad;
    const RING_SQUASH: f32 = 0.38;

    // Precompute orbiting moons (mx, my, radius, depth, seed).
    let (moons, nmoon) = if style.moons { moon_ring(ct, seed, angle) } else { (Default::default(), 0) };

    let lod = Lod::for_disc(rad);
    // Past the terminator `shade` bottoms out at the 0.10 ambient floor and the
    // output snaps to 22 levels, roughly 3 of which are reachable — the fine
    // octaves and the whole cloud deck cannot survive that, so they are not
    // computed there. Lightning fires at a seeded point anywhere on the disc and
    // lights the deck when it does, so those types opt out wholesale; aurora is
    // confined to a polar band, so it opts out by latitude below rather than
    // excluding every type that merely has one.
    let night_ok = style.feat & F_NIGHT_LOD != 0 && ct.lightning == 0.0 && ct.base != Base::Emissive;
    // Functions of `angle` and `seed` alone — hoisted out of the pixel loop.
    let (cs, cc) = (angle * 2.0).sin_cos();
    let morph = angle.sin() * 0.6;
    let swirl_phase = (angle * 0.6).sin() * 1.6 * ct.storm_cells;
    let vortex = vortex_centres(seed, ofs);

    // A sprite is empty off the disc and off a ringed world's ring ellipse, so a
    // row need only be walked across those. Moons orbit out in the margin, so a
    // frame that draws them opts out.
    let narrow = fr.sprite && nmoon == 0;
    // Half-width of the covered band at row offset `ny`, in disc radii.
    let cover = |ny: f32| {
        let disc = (1.0 - ny * ny).max(0.0).sqrt();
        if ct.rings {
            let rr = (ct.ring_outer * ct.ring_outer - (ny / RING_SQUASH).powi(2)).max(0.0).sqrt();
            disc.max(rr)
        } else {
            disc
        }
    };

    let [clip_x0, clip_y0, clip_x1, clip_y1] = fr.clip;
    for iy in clip_y0..clip_y1 {
        let ny = (cy - (iy as f32 + 0.5)) / rad;
        // Clear whatever of the clip the narrowing leaves uncovered — a reused
        // buffer must not show the previous body there.
        let (mut x0, mut x1) = (clip_x0, clip_x1);
        if narrow {
            let half = cover(ny) * rad + 1.0; // +1 px of slack for the rounding
            x0 = clip_x0.max((cx - half).floor().max(0.0) as u32);
            x1 = clip_x1.min((cx + half).ceil().clamp(0.0, size as f32) as u32);
            let row = (iy * size * 4) as usize;
            let span = |a: u32, b: u32| row + (a * 4) as usize..row + (b * 4) as usize;
            if x1 <= x0 {
                out[span(clip_x0, clip_x1)].fill(0);
                continue;
            }
            out[span(clip_x0, x0)].fill(0);
            out[span(x1, clip_x1)].fill(0);
        }
        for ix in x0..x1 {
            let nx = (ix as f32 + 0.5 - cx) / rad;
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
                let night = night_ok && diff <= 0.0 && (ct.aurora == 0.0 || sy.abs() < 0.52);
                let (mut col, emis) = surface(
                    ct,
                    sx,
                    sy,
                    sz,
                    ofs,
                    angle,
                    if night { lod.capped(NIGHT_OCT) } else { lod },
                    style.feat,
                );
                if ct.clouds > 0.0 && !night {
                    // Clouds drift over the surface (2x = parallax, loops) and
                    // slowly billow — a periodic morph reveals new cloud structure
                    // so weather forms and dissipates rather than sliding rigidly.
                    let mut cx3 = nx * cc + nz * cs + ofs[0];
                    let mut cz3 = -nx * cs + nz * cc + ofs[2];

                    // Rotating storm cells: swirl the cloud field around a couple
                    // of seeded vortex centers, spinning with the animation.
                    if ct.storm_cells > 0.0 {
                        for (vx, vz) in vortex {
                            let (dx, dz) = (cx3 - vx, cz3 - vz);
                            let d2v = dx * dx + dz * dz;
                            // exp(-2.2·d²) is under 1e-4 past here and only
                            // scales a rotation angle, so the eddy does nothing.
                            // Most of a disc is this far out.
                            if d2v > 4.2 {
                                continue;
                            }
                            let fall = (-d2v * 2.2).exp();
                            // Bounded (periodic) swirl: the eddy churns back and forth
                            // rather than winding into ever-tighter rings as `angle`
                            // grows unbounded on the continuously-running web.
                            let (ss, sc) = (fall * swirl_phase).sin_cos();
                            cx3 = vx + dx * sc - dz * ss;
                            cz3 = vz + dx * ss + dz * sc;
                        }
                    }

                    let n = lod.oct(2.8, 4);
                    let dens = |ox: f32, oz: f32| {
                        fbm((cx3 + ox) * 2.8, ny * 2.8 + ofs[1] + morph, (cz3 + oz) * 2.8 + morph, n)
                    };
                    // Wispy, fractal cloud tops (domain-warped) so they break
                    // into ragged fronts instead of round blobs. Shadow uses the
                    // cheap plain density.
                    let cloud = fbm_warp(
                        cx3 * 2.8,
                        ny * 2.8 + ofs[1] + morph,
                        cz3 * 2.8 + morph,
                        warp_oct(style.feat, n),
                        n,
                        0.9,
                    );
                    let shadow = smoothstep(0.55, 0.72, dens(l[0] * 0.45, l[2] * 0.45));
                    if style.feat & F_CLOUD_SHADOW != 0 {
                        let sh = 1.0 - 0.22 * shadow * ct.clouds;
                        col = [col[0] * sh, col[1] * sh, col[2] * sh];
                    }
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
                    // `ndh^shininess` collapses fast, so over most of the disc
                    // the glint cannot show at all. `shimmer <= 1`, so bound the
                    // whole term first and skip its fBm where it can't.
                    let peak = ndh.powf(ct.shininess) * ct.specular * mat;
                    if peak > SPEC_FLOOR {
                        // Cycling shimmer so water/ice glints twinkle over time.
                        let shimmer =
                            0.82 + 0.18 * fbm(sx * 5.0 + angle * 2.5, sy * 5.0, sz * 5.0, lod.oct(5.0, 2));
                        let sp = peak * shimmer;
                        o[0] = clamp01(o[0] + sp);
                        o[1] = clamp01(o[1] + sp);
                        o[2] = clamp01(o[2] + sp);
                    }
                }
                if has_atmo && style.feat & F_ATMO != 0 {
                    // `powf(3.0)` is a full exp/log even for a literal exponent.
                    let rim = (1.0 - nz).powi(3) * 0.6;
                    o[0] = clamp01(o[0] + ct.atmo[0] * rim);
                    o[1] = clamp01(o[1] + ct.atmo[1] * rim);
                    o[2] = clamp01(o[2] + ct.atmo[2] * rim);
                }
            } else if fr.sprite {
                // Off the disc a tile is empty — the scene shows through.
                o = [0.0, 0.0, 0.0];
                a = 0.0;
            } else {
                let s = if style.feat & F_STARFIELD != 0 { star_bg(ix, iy, seed) } else { [0, 0, 0, 255] };
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

// ---------------------------------------------------------------------------
// WebGL2 port
// ---------------------------------------------------------------------------

/// The uniform block and the GLSL source the browser's GPU path needs.
///
/// **Browser-only, and gated so that it is.** Compiling it into the native build
/// changed nothing about what the CPU renderer computes — and still moved
/// `out/moon_*.png` by up to 4/255 across 5% of its pixels, because another
/// caller of `Lod::oct`/`moon_ring`/`seed_offsets` in the same LTO unit is
/// enough to re-price their inlining and re-contract a multiply-add. The
/// generators have no GPU, so the honest fix is for them not to carry it.
#[cfg(any(feature = "gl", test))]
mod gl;
#[cfg(any(feature = "gl", test))]
pub use gl::{gl_tile_uniforms, gl_uniforms, GL_SHADER, GL_SOURCES, GL_UNIFORMS_LEN};

#[cfg(test)]
mod band_tests {
    use super::*;

    /// Splitting a frame across workers is only sound if a band is bit-identical
    /// to the same rows of the whole frame — every worker shades from the same
    /// pure function of pixel position, and nothing accumulates across rows.
    #[test]
    fn render_band_matches_whole() {
        const SIZE: u32 = 96;
        let n = (SIZE * SIZE * 4) as usize;
        for &t in &[0usize, 7, 8, 10, 20, 24] {
            let p: Vec<f32> = (0..NUM_PARAMS).map(|i| param(t, i as u32)).collect();
            let feat = F_ALL | F_NIGHT_LOD;
            let mut whole = vec![0u8; n];
            render_rgba_features(SIZE, t, 1, 0.7, &p, 0, 0.7, 1, feat, &mut whole);

            // Uneven bands on purpose: a real pool hands out a remainder.
            for bands in [2u32, 3, 5, 7] {
                let mut out = vec![0u8; n];
                let step = SIZE.div_ceil(bands);
                for b in 0..bands {
                    let (y0, y1) = (b * step, ((b + 1) * step).min(SIZE));
                    if y0 >= y1 {
                        continue;
                    }
                    let mut band = vec![0u8; n];
                    render_rgba_band(SIZE, t, 1, 0.7, &p, 0, 0.7, 1, feat, y0, y1, &mut band);
                    let r = (y0 * SIZE * 4) as usize..(y1 * SIZE * 4) as usize;
                    out[r.clone()].copy_from_slice(&band[r]);
                }
                assert_eq!(out, whole, "type {t}, {bands} bands");
            }
        }
    }
}
