//! moon — a procedural, seed-driven **planet with its own moons**.
//!
//! Pure math over the dependency-free `*-core` rlibs. Where `solar` renders a
//! whole star system into a draggable viewport, `moon` scopes that same depth-
//! sorted compositor down to a single stage: one lit parent planet at the centre
//! and 2–5 satellites orbiting it, each correctly passing IN FRONT OF and BEHIND
//! the parent body as it goes round. Same seed => the same planet + moons, forever.
//!
//! This crate leans on the workspace's shared, dependency-free `*-core` rlibs:
//! `noise-core`/`dither-core` for the noise/color/dither primitives, `scene-core`
//! for the compositor helpers, and `planet-core` for the parent world itself —
//! the very same archetypes and shader the `planet` demo shows head-on and
//! `solar` puts in orbit, asked for as a sprite tile. What stays local is the
//! fake-3D cratered-rock tile for the moons (a genuinely different surface: no
//! atmosphere, no weather, all regolith and impact craters) and the layer on top
//! — orbital layout for the satellites and the depth sort that makes a moon on
//! the far side of its orbit disappear behind the planet while one on the near
//! side draws over it.
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
// Shared primitives — noise/color math and the ordered dither, used below by
// the moon shader and the starfield backdrop.
// ===========================================================================

use dither_core::bayer;
use noise_core::{clamp01, contrast, fbm, hash3, mix, smoothstep, worley, Rgb};
use scene_core::{blit, to_screen, Rng, Tile, ORBIT_FLATTEN};

// The shared deep-space backdrop, common to every scene crate.
use background_core::{paint_backdrop, paint_stars, Backdrop, StarLayer, StarTints, Starfield};

// `Camera` is part of this crate's public API (its bin and wasm face import it as
// `moon::Camera` / `crate::Camera`), so re-export scene-core's — a plain `pub use`
// also brings it into scope for the lib below.
pub use scene_core::Camera;

/// Bounded, decorrelated per-body noise offsets — keep them small so f32
/// precision holds and the noise doesn't collapse into bands. Thin wrapper over
/// `noise_core::seed_offsets` pinned to this crate's historical span (220.0) so
/// every existing call site yields the exact same numbers.
fn seed_offsets(seed: u32) -> [f32; 3] {
    noise_core::seed_offsets(seed, 220.0)
}

// ===========================================================================
// Parent planet roster
// ===========================================================================

/// The worlds that can sit at the centre of the stage, named as `planet-core`
/// types — that crate owns the palette and the shader, exactly as it does for
/// `planet`'s hero view and `solar`'s orbiting bodies. One is chosen per system
/// by seed. Kept deliberately short: the parent is a backdrop for the moons
/// crossing it, so the roster favours worlds that read at a glance.
///
/// The web demo's `PARENT_NAMES` is index-aligned with this list (it can't read
/// `&str`s across the C ABI), so reordering here means editing `web/index.html`.
const PARENTS: &[&str] = &["terran", "ocean", "barren", "gas_giant", "ice_giant"];

/// `planet-core` type index for parent `i`. Resolved once at generation time and
/// cached on the [`MoonSystem`], never per frame.
fn parent_type(i: usize) -> usize {
    planet_core::type_index(PARENTS[i % PARENTS.len()]).unwrap_or(0)
}

/// Number of parent-planet archetypes.
pub fn parent_kind_count() -> usize {
    PARENTS.len()
}
/// Name of a parent archetype (wraps out of range).
pub fn parent_kind_name(i: usize) -> &'static str {
    PARENTS[i % PARENTS.len()]
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
    pub parent_kind: usize, // index into PARENTS (what the HUD names)
    pub parent_type: usize, // index into planet_core::TYPES (what renders it)
    pub parent_radius: f32, // world units
    pub parent_spin: f32,   // parent axial-spin turns per unit time
    pub moons: Vec<Moon>,
    pub orbit_width: f32,   // dashed orbit line thickness, px (1..=6)
    // Freeze the parent's weather into a baked map rather than evaluating it per
    // pixel per frame. The parent fills the view here, so it is the whole cost of
    // the demo. See `planet_core::F_BAKED_CLOUDS`.
    //
    // OFF by default so `out/moon.gif` keeps the animated deck and stays
    // byte-identical; the web demo turns it on at construction.
    pub frozen_clouds: bool,
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
        let parent_type = parent_type(parent_kind);
        // Gas/ice giants are bigger discs than rocky/terran parents.
        let parent_radius = if planet_core::is_giant(parent_type) {
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

        MoonSystem { seed, parent_kind, parent_type, parent_radius, parent_spin, moons, orbit_width: 1.0, frozen_clouds: false }
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
                // One angle turns the surface and advances the weather alike —
                // that is the planet shader's contract.
                let spin_a = self.parent_spin * t * TAU;
                let tile = render_parent_tile(self.parent_type, self.seed, spin_a, rad_render, self.frozen_clouds);
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

/// Ordered-dither quantize to kill banding while staying crisp under motion.
/// Thin wrapper over `dither_core::quant` pinned to this crate's historical
/// 24 levels / 0.7 dither strength; the Bayer bias comes from `dither_core::bayer`.
fn quant(o: Rgb, bx: f32) -> Rgb {
    dither_core::quant(o, bx, 24.0, 0.7)
}

/// Render the parent planet to a lit RGBA tile of diameter ~`2*rad_px`. A thin
/// wrapper over `planet_core::render_tile` — the same call `solar` makes for the
/// worlds in its orbits, and the same shader the `planet` demo shows head-on;
/// moon only pins the light to its fixed off-screen sun (`LIGHT_DIR`).
fn render_parent_tile(type_idx: usize, seed: u32, spin_a: f32, rad_px: f32, frozen: bool) -> Tile {
    let feat = planet_core::F_ALL
        | if frozen { planet_core::F_BAKED_CLOUDS | planet_core::F_BAKED_SURFACE | planet_core::F_BAKED_BANDS } else { 0 };
    planet_core::render_tile_features(type_idx, seed, spin_a, LIGHT_DIR, rad_px, true, feat)
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

/// This scene's sky: mostly pale/blue-white, a few warm.
const STAR_TINTS: StarTints = &[
    (0.50, [0.92, 0.95, 1.00]),
    (0.72, [0.72, 0.83, 1.00]),
    (0.90, [1.00, 0.96, 0.78]),
    (1.01, [1.00, 0.82, 0.60]),
];

/// One static layer: this scene has a fixed camera, so there is no pan to be
/// parallax *for*, and a single grid keyed by the seed is the whole sky.
const STAR_LAYERS: &[StarLayer] = &[
    StarLayer { parallax: 0.0, spacing: 9.0, threshold: 0.86, brightness: 1.0, faint: 0.45, salt: 0 },
];

/// Dithered navy, no nebula: the parent planet fills the frame, so the backdrop
/// stays flat behind it.
const BACKDROP: Backdrop = Backdrop {
    base: [0.028, 0.026, 0.060],
    dither: 0.010,
    nebula: None,
};

/// Paint the dark space backdrop plus a faint, fixed starfield.
fn paint_background(out: &mut [u8], w: u32, h: u32, seed: u32) {
    paint_backdrop(out, w, h, &BACKDROP, seed, 0.0, 0.0, 1.0, 0.0, None);
    let sky = Starfield::new(STAR_LAYERS, STAR_TINTS);
    let salt = seed as i32 ^ 0x51ed;
    paint_stars(out, w, h, &sky, 0.0, 0.0, |cx, cy, _| hash3(cx, cy, salt));
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

// Browser (wasm) C-ABI glue — excluded from native builds. See wasm.rs.
#[cfg(target_arch = "wasm32")]
mod wasm;

#[cfg(test)]
mod tests {
    use super::*;

    /// `PARENTS` names `planet-core` archetypes as strings, so a typo (or a
    /// renamed row over there) would silently fall back to type 0 and quietly
    /// turn every parent world into a terran. Pin it.
    #[test]
    fn every_parent_name_resolves() {
        for (i, name) in PARENTS.iter().enumerate() {
            assert!(
                planet_core::type_index(name).is_some(),
                "no planet-core type named {name:?} (PARENTS[{i}])"
            );
        }
    }

    /// The generator gives giants a bigger disc, so at least one parent has to
    /// be one — otherwise that branch is dead and every scene reads the same.
    #[test]
    fn the_roster_has_a_giant_and_a_rocky_world() {
        let giants = PARENTS.iter().enumerate().filter(|(i, _)| planet_core::is_giant(parent_type(*i)));
        assert!(giants.clone().count() > 0, "no giant parent");
        assert!(giants.count() < PARENTS.len(), "no rocky parent");
    }
}
