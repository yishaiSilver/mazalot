//! planet-core — the single source of truth for procedural planet generation.
//!
//! Pure math, zero dependencies. Produces raw RGBA bytes via [`render_rgba`];
//! callers wrap those however they like (the native crate turns them into
//! GIFs/PNGs with the `image` crate; the web crate exposes them to a canvas).
//!
//! A planet TYPE is a [`PType`] row (palette + thresholds + flags). Four... five
//! base algorithms render everything: Terrestrial, Cratered, Banded, Emissive,
//! Cloudy. Rings and specular "glare" are reusable modifiers. Same inputs =>
//! same planet. The "3D" is per-pixel sphere math: rotate the surface point
//! around Y and sample 3D noise there, shade against a fixed light.

// ---------------------------------------------------------------------------
// Noise: 3D value-noise fBm + 3D Worley (cellular) for craters.
// ---------------------------------------------------------------------------

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

/// 3D Worley F1: distance to nearest hashed feature point. ~[0, 1.0].
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

// ---------------------------------------------------------------------------
// Color helpers
// ---------------------------------------------------------------------------

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
    PType { name: "terran", base: Base::Terrestrial, freq: 2.0, contrast: 2.1, stops: TERRAN, clouds: 0.85, caps: 0.9, atmo: [0.30, 0.45, 0.65], specular: 0.22, shininess: 8.0, ..base() },
    PType { name: "ocean", base: Base::Terrestrial, freq: 2.2, contrast: 1.7, stops: OCEAN, clouds: 0.7, caps: 0.7, atmo: [0.25, 0.42, 0.66], specular: 0.32, shininess: 7.0, ..base() },
    PType { name: "archipelago", base: Base::Terrestrial, freq: 4.0, contrast: 1.6, stops: ARCHIPELAGO, clouds: 0.5, caps: 0.3, atmo: [0.24, 0.48, 0.62], specular: 0.26, shininess: 8.0, ..base() },
    PType { name: "desert", base: Base::Terrestrial, freq: 2.4, contrast: 1.5, stops: DESERT, clouds: 0.12, caps: 0.15, atmo: [0.38, 0.28, 0.18], specular: 0.04, shininess: 24.0, ..base() },
    PType { name: "swamp", base: Base::Terrestrial, freq: 2.6, contrast: 1.6, stops: SWAMP, clouds: 0.6, caps: 0.0, atmo: [0.24, 0.34, 0.20], specular: 0.12, shininess: 9.0, ..base() },
    PType { name: "iron", base: Base::Terrestrial, freq: 2.2, contrast: 1.9, stops: IRON, clouds: 0.0, caps: 0.1, atmo: [0.42, 0.20, 0.12], specular: 0.06, shininess: 20.0, ..base() },
    // family E — ice shell (terrestrial + ridged): noticeable icy sheen
    PType { name: "ice", base: Base::Terrestrial, freq: 2.6, contrast: 1.4, ridged: true, stops: ICE, clouds: 0.2, caps: 0.0, atmo: [0.45, 0.60, 0.85], specular: 0.45, shininess: 14.0, ..base() },
    // family B — cratered (light=highland, dark=maria): matte dust
    PType { name: "barren", base: Base::Cratered, freq: 5.0, light: [0.55, 0.55, 0.58], dark: [0.20, 0.20, 0.23], specular: 0.0, shininess: 24.0, ..base() },
    // family C — banded (gas isn't shiny: soft, broad)
    PType { name: "gas_giant", base: Base::Banded, light: [0.86, 0.77, 0.60], dark: [0.55, 0.40, 0.28], bands: 11.0, turb: 0.6, specular: 0.05, shininess: 6.0, ..base() },
    PType { name: "ice_giant", base: Base::Banded, light: [0.55, 0.72, 0.90], dark: [0.22, 0.38, 0.68], bands: 8.0, turb: 0.35, atmo: [0.30, 0.45, 0.70], specular: 0.08, shininess: 8.0, ..base() },
    // family D — emissive (self-lit; little/no glare)
    PType { name: "lava", base: Base::Emissive, rock: [0.16, 0.09, 0.07], glow_lo: [1.0, 0.42, 0.06], glow_hi: [1.0, 0.92, 0.35], glow_e0: 0.44, glow_e1: 0.66, freq: 3.0, specular: 0.05, shininess: 20.0, ..base() },
    PType { name: "fungal", base: Base::Emissive, rock: [0.10, 0.10, 0.14], glow_lo: [0.15, 0.85, 0.75], glow_hi: [0.65, 0.35, 0.95], glow_e0: 0.50, glow_e1: 0.72, freq: 3.2, atmo: [0.14, 0.32, 0.34], specular: 0.0, shininess: 24.0, ..base() },
    // --- second batch ---
    PType { name: "savanna", base: Base::Terrestrial, freq: 2.2, contrast: 1.6, stops: SAVANNA, clouds: 0.25, caps: 0.10, atmo: [0.40, 0.35, 0.20], specular: 0.04, shininess: 24.0, ..base() },
    PType { name: "gaia", base: Base::Terrestrial, freq: 2.2, contrast: 1.7, stops: GAIA, clouds: 0.60, caps: 0.20, atmo: [0.30, 0.50, 0.35], specular: 0.09, shininess: 9.0, ..base() },
    PType { name: "tundra", base: Base::Terrestrial, freq: 2.4, contrast: 1.6, stops: TUNDRA, clouds: 0.30, caps: 0.90, atmo: [0.50, 0.60, 0.75], specular: 0.35, shininess: 12.0, ..base() },
    PType { name: "alpine", base: Base::Terrestrial, freq: 2.6, contrast: 2.6, stops: ALPINE, clouds: 0.40, caps: 0.50, atmo: [0.40, 0.50, 0.70], specular: 0.14, shininess: 12.0, ..base() },
    PType { name: "obsidian", base: Base::Terrestrial, freq: 2.4, contrast: 1.8, stops: OBSIDIAN, clouds: 0.0, caps: 0.0, atmo: [0.20, 0.15, 0.30], specular: 0.55, shininess: 30.0, ..base() },
    PType { name: "chrome", base: Base::Terrestrial, freq: 2.2, contrast: 2.0, stops: CHROME, clouds: 0.0, caps: 0.0, atmo: [0.30, 0.35, 0.45], specular: 0.95, shininess: 32.0, ..base() },
    // more cratered
    PType { name: "moon", base: Base::Cratered, freq: 4.0, light: [0.62, 0.62, 0.60], dark: [0.28, 0.28, 0.30], specular: 0.0, shininess: 24.0, ..base() },
    // more banded + ringed
    PType { name: "storm_giant", base: Base::Banded, light: [0.80, 0.55, 0.45], dark: [0.45, 0.22, 0.20], bands: 9.0, turb: 1.1, specular: 0.04, shininess: 6.0, ..base() },
    PType { name: "ringed_giant", base: Base::Banded, light: [0.82, 0.74, 0.58], dark: [0.50, 0.40, 0.30], bands: 10.0, turb: 0.5,
            rings: true, ring_inner: 1.30, ring_outer: 2.20, ring_col: [0.82, 0.74, 0.58], radius_scale: 0.50, specular: 0.05, shininess: 6.0, ..base() },
    // more emissive
    PType { name: "molten_sea", base: Base::Emissive, rock: [0.25, 0.10, 0.06], glow_lo: [1.0, 0.35, 0.05], glow_hi: [1.0, 0.85, 0.40], glow_e0: 0.30, glow_e1: 0.55, freq: 2.6, atmo: [0.30, 0.10, 0.05], specular: 0.06, shininess: 18.0, ..base() },
    PType { name: "radioactive", base: Base::Emissive, rock: [0.10, 0.14, 0.08], glow_lo: [0.40, 0.90, 0.20], glow_hi: [0.80, 1.0, 0.40], glow_e0: 0.50, glow_e1: 0.72, freq: 3.0, atmo: [0.20, 0.40, 0.10], specular: 0.0, shininess: 24.0, ..base() },
    PType { name: "crystal", base: Base::Emissive, rock: [0.15, 0.10, 0.20], glow_lo: [0.50, 0.30, 0.90], glow_hi: [0.70, 0.90, 1.0], glow_e0: 0.55, glow_e1: 0.68, freq: 3.5, atmo: [0.30, 0.25, 0.50], specular: 0.45, shininess: 30.0, ..base() },
    // family E — cloud-shrouded (soft diffuse glare)
    PType { name: "toxic", base: Base::Cloudy, light: [0.85, 0.82, 0.45], dark: [0.55, 0.60, 0.25], bands: 6.0, turb: 1.0, atmo: [0.50, 0.50, 0.20], specular: 0.06, shininess: 6.0, ..base() },
    PType { name: "storm_shroud", base: Base::Cloudy, light: [0.85, 0.86, 0.90], dark: [0.45, 0.48, 0.55], bands: 5.0, turb: 1.2, atmo: [0.40, 0.45, 0.55], specular: 0.08, shininess: 6.0, ..base() },
];

/// Number of planet types.
pub fn type_count() -> usize {
    TYPES.len()
}
/// Name of a planet type (wraps on out-of-range index).
pub fn type_name(i: usize) -> &'static str {
    TYPES[i % TYPES.len()].name
}

// ---------------------------------------------------------------------------
// Surface shading
// ---------------------------------------------------------------------------

/// Bounded, decorrelated noise offsets from a seed. These MUST stay small:
/// huge sample coordinates lose f32 precision and the noise collapses into
/// horizontal bands (the "circular planet" bug with large random seeds).
fn seed_offsets(seed: u32) -> [f32; 3] {
    [
        hash3(seed as i32, 1, 7) * 256.0 + 4.0,
        hash3(seed as i32, 2, 7) * 256.0 + 4.0,
        hash3(seed as i32, 3, 7) * 256.0 + 4.0,
    ]
}

fn surface(ct: &PType, sx: f32, sy: f32, sz: f32, ofs: [f32; 3]) -> (Rgb, f32) {
    let (px, py, pz) = (sx + ofs[0], sy + ofs[1], sz + ofs[2]);
    match ct.base {
        Base::Terrestrial => {
            let raw = fbm(px * ct.freq, py * ct.freq, pz * ct.freq, if ct.ridged { 5 } else { 6 });
            let n = if ct.ridged { 1.0 - (2.0 * raw - 1.0).abs() } else { raw };
            let h = contrast(n, ct.contrast);
            let mut col = ramp(ct.stops, h);
            let cap = smoothstep(0.72, 0.9, sy.abs()) * ct.caps;
            col = mix(col, [0.92, 0.95, 1.0], cap);
            (col, 0.0)
        }
        Base::Cratered => {
            let m = smoothstep(0.4, 0.6, fbm(px * 1.2, py * 1.2, pz * 1.2, 5));
            let base_col = mix(ct.dark, ct.light, m);
            let w = worley(px * ct.freq, py * ct.freq, pz * ct.freq);
            let bowl = smoothstep(0.0, 0.35, w);
            let rim = smoothstep(0.30, 0.42, w) * (1.0 - smoothstep(0.42, 0.60, w));
            let col = [
                clamp01(base_col[0] * (0.55 + 0.45 * bowl) + rim * 0.30),
                clamp01(base_col[1] * (0.55 + 0.45 * bowl) + rim * 0.30),
                clamp01(base_col[2] * (0.55 + 0.45 * bowl) + rim * 0.30),
            ];
            (col, 0.0)
        }
        Base::Banded => {
            let warp = fbm(px * 1.3, py * 1.3, pz * 1.3, 5);
            let lat = sy + (warp - 0.5) * ct.turb;
            let band = 0.5 + 0.5 * (lat * ct.bands).sin();
            let mut col = mix(ct.dark, ct.light, band);
            let fine = fbm(px * 4.0, py * 4.0, pz * 4.0, 4);
            col = mix(col, ct.light, smoothstep(0.55, 0.8, fine) * 0.35);
            (col, 0.0)
        }
        Base::Emissive => {
            let n = contrast(fbm(px * ct.freq, py * ct.freq, pz * ct.freq, 6), 1.7);
            let glow = smoothstep(ct.glow_e0, ct.glow_e1, n);
            let gcol = mix(ct.glow_lo, ct.glow_hi, smoothstep(ct.glow_e0 + 0.1, ct.glow_e1 + 0.15, n));
            (mix(ct.rock, gcol, glow), glow)
        }
        Base::Cloudy => {
            let t = fbm(px * 2.0, py * 2.0, pz * 2.0, 6);
            let band = 0.5 + 0.5 * (sy * ct.bands + (t - 0.5) * 6.0 * ct.turb).sin();
            (mix(ct.dark, ct.light, clamp01(band * 0.6 + t * 0.4)), 0.0)
        }
    }
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

/// A tweakable parameter of a type, for the web sliders.
/// 0 = contrast, 1 = frequency, 2 = specular, 3 = shininess.
pub fn param(type_idx: usize, which: u32) -> f32 {
    let ct = &TYPES[type_idx % TYPES.len()];
    match which {
        0 => ct.contrast,
        1 => ct.freq,
        2 => ct.specular,
        3 => ct.shininess,
        _ => 0.0,
    }
}

/// Render one planet frame as RGBA into `out` (must be >= size*size*4 bytes).
/// `angle` is the rotation in radians; a full 2π loop is seamless.
pub fn render_rgba(size: u32, type_idx: usize, seed: u32, angle: f32, out: &mut [u8]) {
    render_ct(size, &TYPES[type_idx % TYPES.len()], seed, angle, out);
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
    render_ct(size, &ct, seed, angle, out);
}

fn render_ct(size: u32, ct: &PType, seed: u32, angle: f32, out: &mut [u8]) {
    let (cx, cy) = (size as f32 / 2.0, size as f32 / 2.0);
    let ofs = seed_offsets(seed);
    let l = {
        let v = [-0.55, 0.45, 0.70f32];
        let m = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
        [v[0] / m, v[1] / m, v[2] / m]
    };
    let (sina, cosa) = angle.sin_cos();
    let has_atmo = ct.atmo != [0.0; 3];
    let rad = (size as f32 * 27.0 / 64.0) * ct.radius_scale;
    const RING_SQUASH: f32 = 0.38;

    for iy in 0..size {
        for ix in 0..size {
            let nx = (ix as f32 + 0.5 - cx) / rad;
            let ny = (cy - (iy as f32 + 0.5)) / rad;
            let d2 = nx * nx + ny * ny;

            let mut o;
            if d2 <= 1.0 {
                let nz = (1.0 - d2).sqrt();
                let sx = nx * cosa + nz * sina;
                let sy = ny;
                let sz = -nx * sina + nz * cosa;

                let (mut col, emis) = surface(ct, sx, sy, sz, ofs);
                if ct.clouds > 0.0 {
                    let (cs, cc) = (angle * 1.25).sin_cos();
                    let cx3 = nx * cc + nz * cs + ofs[0];
                    let cz3 = -nx * cs + nz * cc + ofs[2];
                    let cloud = fbm(cx3 * 2.8, ny * 2.8 + ofs[1], cz3 * 2.8, 4);
                    col = mix(col, [1.0, 1.0, 1.0], smoothstep(0.55, 0.72, cloud) * ct.clouds);
                }
                let diff = (nx * l[0] + ny * l[1] + nz * l[2]).max(0.0);
                let shade = (0.10 + 0.90 * diff).max(emis);
                o = [col[0] * shade, col[1] * shade, col[2] * shade];
                if ct.specular > 0.0 {
                    let hm = ((l[0]).powi(2) + (l[1]).powi(2) + (l[2] + 1.0).powi(2)).sqrt();
                    let ndh = (nx * l[0] / hm + ny * l[1] / hm + nz * (l[2] + 1.0) / hm).max(0.0);
                    let sp = ndh.powf(ct.shininess) * ct.specular;
                    o[0] = clamp01(o[0] + sp);
                    o[1] = clamp01(o[1] + sp);
                    o[2] = clamp01(o[2] + sp);
                }
                if has_atmo {
                    let rim = (1.0 - nz).powf(3.0) * 0.6;
                    o[0] = clamp01(o[0] + ct.atmo[0] * rim);
                    o[1] = clamp01(o[1] + ct.atmo[1] * rim);
                    o[2] = clamp01(o[2] + ct.atmo[2] * rim);
                }
            } else {
                let s = star_bg(ix, iy, seed);
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
                    o = [lerp(o[0], rc[0], alpha), lerp(o[1], rc[1], alpha), lerp(o[2], rc[2], alpha)];
                }
            }

            let idx = ((iy * size + ix) * 4) as usize;
            out[idx] = (clamp01(o[0]) * 255.0) as u8;
            out[idx + 1] = (clamp01(o[1]) * 255.0) as u8;
            out[idx + 2] = (clamp01(o[2]) * 255.0) as u8;
            out[idx + 3] = 255;
        }
    }
}
