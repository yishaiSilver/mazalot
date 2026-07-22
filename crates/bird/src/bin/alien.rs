//! Procedural ALIEN avian generator — structural randomness, not just recolor.
//!
//! Each creature independently rolls a body plan AND a set of features, so the
//! variety is combinatorial rather than "same shape, new palette":
//!   eyes (1-3, row or column) · mouth type (beak/hook/tube/mandible/maw)
//!   head gear (antennae/horns/crest/frill) · dorsal spikes · tail form
//!   leg count/type · wing stub · body pattern (spots/stripes/dorsal/irid)
//!   alien palette (any hue, complementary/triadic accents)
//!
//! Lower-res grid => chunkier pixels. Same seed => same creature. Zero assets.
//!
//! ─────────────────────────────────────────────────────────────────────────
//! RANDOMIZATION TABLE  (G = per-genus/family, I = per-individual)
//! ─────────────────────────────────────────────────────────────────────────
//! CHARACTERISTIC          RANDOMIZED AS                              SCOPE
//! body plan               2 of 20 archetypes + blend weight          G
//! blend weight            0.12–0.35 or 0.65–0.88 (extreme-biased)    G
//! hue                     genus hue ± spread(0.02–0.10)              G→I
//! saturation / value      genus ranges                               G
//! color scheme            complementary / triadic / analogous        G
//! body cy / rx / ry       ±0.5px / ×0.92–1.08                        I
//! head ry                 ×0.90–1.05                                 I
//! mouth type              plan bias, reroll ~mutation                 I
//! eye count               1 / 2 / 3                                   I
//! eye layout              row or column (genus prob)                  G→I
//! eye radius              genus range                                 I
//! headgear type           none/antennae/horns/crest/frill            G
//! headgear count (gear_n) 1–3                                         I
//! pattern                 solid/spots/stripes/dorsal/irid             G
//! tail type               plan bias, reroll ~mutation                 I
//! leg count               0 / 2 / 3 (from plan blend)                 I
//! tentacle legs           genus probability                          G→I
//! wing / membrane / eyestalk / lure  blended appendage biases        G→I
//! wing span / angle / struts         0.8–1.45 / −0.35–0.6 / 2–4      I
//! eyestalk count / len / splay       1–3 / 3.0–7.5 / 0.2–0.55        I
//! lure reach / arc                   0.85–1.6 / 2.5–6.5              I
//! horn len / angle                   2.5–4.8 / −0.5–0.5              I
//! crest count / len                  3–7 / 1.8–3.6                   I
//! antenna len / splay                3.5–7.5 / 1.8–4.5               I
//! frill size                         0.8–1.4                         I
//! pincer hook len                    1.5–3.2                         I
//! dorsal spike count                 3–8                             I
//! maw teeth count                    2–5                             I
//! fan feather count                  3–7                             I
//! anim: bob/blink/sway/wag/step/flap amplitudes                      I
//! ─────────────────────────────────────────────────────────────────────────

use image::codecs::gif::{GifEncoder, Repeat};
use image::{imageops, Delay, Frame, Rgba, RgbaImage};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::f32::consts::TAU;
use std::fs::File;

const GRID: u32 = 30; // small on purpose -> more pixelated
const SHEET_UP: u32 = 4;
const ANIM_FRAMES: u32 = 6; // idle-loop length — set to 5, 8, whatever you like

type Rgb = [f32; 3];

// ---------------- helpers ----------------
fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [a[0] + (b[0] - a[0]) * t, a[1] + (b[1] - a[1]) * t, a[2] + (b[2] - a[2]) * t]
}
fn clamp01(x: f32) -> f32 {
    x.max(0.0).min(1.0)
}
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp01((x - e0) / (e1 - e0));
    t * t * (3.0 - 2.0 * t)
}
fn to_rgba(c: Rgb) -> Rgba<u8> {
    Rgba([(clamp01(c[0]) * 255.0) as u8, (clamp01(c[1]) * 255.0) as u8, (clamp01(c[2]) * 255.0) as u8, 255])
}
fn hsv(h: f32, s: f32, v: f32) -> Rgb {
    let h = (h.fract() + 1.0).fract() * 6.0;
    let i = h.floor() as i32 % 6;
    let f = h - h.floor();
    let (p, q, t) = (v * (1.0 - s), v * (1.0 - s * f), v * (1.0 - s * (1.0 - f)));
    match i {
        0 => [v, t, p],
        1 => [q, v, p],
        2 => [p, v, t],
        3 => [p, q, v],
        4 => [t, p, v],
        _ => [v, p, q],
    }
}
fn darker(c: Rgb, k: f32) -> Rgb {
    mix(c, [0.04, 0.04, 0.08], k)
}
fn lighter(c: Rgb, k: f32) -> Rgb {
    mix(c, [1.0, 1.0, 1.0], k)
}
fn r(rng: &mut StdRng, a: f32, b: f32) -> f32 {
    a + (b - a) * rng.gen::<f32>()
}
fn seg_dist(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let l2 = dx * dx + dy * dy;
    let t = if l2 <= 0.0 { 0.0 } else { (((px - ax) * dx + (py - ay) * dy) / l2).clamp(0.0, 1.0) };
    ((px - (ax + t * dx)).powi(2) + (py - (ay + t * dy)).powi(2)).sqrt()
}
// tiny 2D value noise for patterns
fn h2(x: i32, y: i32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x8da6_b343) ^ (y as u32).wrapping_mul(0xd816_3841);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2c1b_3c6d);
    h ^= h >> 12;
    (h & 0xffff) as f32 / 65535.0
}
fn vnoise(x: f32, y: f32) -> f32 {
    let (xi, yi) = (x.floor(), y.floor());
    let (fx, fy) = (x - xi, y - yi);
    let (xi, yi) = (xi as i32, yi as i32);
    let (u, v) = (fx * fx * (3.0 - 2.0 * fx), fy * fy * (3.0 - 2.0 * fy));
    let a = h2(xi, yi);
    let b = h2(xi + 1, yi);
    let c = h2(xi, yi + 1);
    let d = h2(xi + 1, yi + 1);
    let ab = a + (b - a) * u;
    let cd = c + (d - c) * u;
    ab + (cd - ab) * v
}

// ---------------- canvas ----------------
struct Canvas {
    col: Vec<Rgb>,
    filled: Vec<bool>,
}
impl Canvas {
    fn new() -> Self {
        Canvas { col: vec![[0.0; 3]; (GRID * GRID) as usize], filled: vec![false; (GRID * GRID) as usize] }
    }
    fn idx(x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 || x >= GRID as i32 || y >= GRID as i32 { None } else { Some((y as u32 * GRID + x as u32) as usize) }
    }
    fn put(&mut self, x: i32, y: i32, c: Rgb) {
        if let Some(i) = Self::idx(x, y) {
            self.col[i] = c;
            self.filled[i] = true;
        }
    }
    fn filledp(&self, x: i32, y: i32) -> bool {
        Self::idx(x, y).map(|i| self.filled[i]).unwrap_or(false)
    }
}

#[derive(Clone, Copy)]
struct Ell {
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
}

const LIGHT: [f32; 3] = [0.32, -0.80, 0.51];

fn shade(base: Rgb, nx: f32, ny: f32, nz: f32) -> Rgb {
    let diff = nx * LIGHT[0] + ny * LIGHT[1] + nz * LIGHT[2];
    let lit = 0.55 + 0.5 * diff;
    if lit > 0.86 {
        lighter(base, 0.24)
    } else if lit < 0.47 {
        darker(base, 0.28)
    } else {
        base
    }
}

#[derive(Clone, Copy)]
enum Pattern {
    Solid,
    Spots,
    Stripes,
    Dorsal,
    Irid,
}

fn body_blob(cv: &mut Canvas, e: Ell, top: Rgb, bot: Rgb, pat: Pattern, patc: Rgb, hue: f32) {
    let x0 = (e.cx - e.rx - 1.0).floor() as i32;
    let x1 = (e.cx + e.rx + 1.0).ceil() as i32;
    let y0 = (e.cy - e.ry - 1.0).floor() as i32;
    let y1 = (e.cy + e.ry + 1.0).ceil() as i32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let nx = (x as f32 + 0.5 - e.cx) / e.rx;
            let ny = (y as f32 + 0.5 - e.cy) / e.ry;
            let r2 = nx * nx + ny * ny;
            if r2 > 1.0 {
                continue;
            }
            let nz = (1.0 - r2).sqrt();
            let vt = smoothstep(0.35, 0.9, (y as f32 + 0.5 - (e.cy - e.ry)) / (2.0 * e.ry));
            let mut base = mix(top, bot, vt);
            match pat {
                Pattern::Solid => {}
                Pattern::Spots => {
                    if vnoise((x as f32) * 0.7 + 3.0, (y as f32) * 0.7) > 0.62 {
                        base = patc;
                    }
                }
                Pattern::Stripes => {
                    if ((x as f32 - y as f32 * 0.4) * 0.9).sin() > 0.35 {
                        base = patc;
                    }
                }
                Pattern::Dorsal => {
                    base = mix(base, patc, smoothstep(0.2, -0.9, ny));
                }
                Pattern::Irid => {
                    // subtle sheen, not a full rainbow sweep
                    base = mix(base, hsv((hue + 0.12 + 0.12 * nx).fract(), 0.6, 0.85), 0.12 + 0.08 * nx);
                }
            }
            cv.put(x, y, shade(base, nx, ny, nz));
        }
    }
}

fn blob(cv: &mut Canvas, e: Ell, top: Rgb, bot: Rgb) {
    body_blob(cv, e, top, bot, Pattern::Solid, top, 0.0);
}

fn capsule(cv: &mut Canvas, ax: f32, ay: f32, bx: f32, by: f32, rad: f32, top: Rgb, bot: Rgb) {
    let x0 = (ax.min(bx) - rad - 1.0).floor() as i32;
    let x1 = (ax.max(bx) + rad + 1.0).ceil() as i32;
    let y0 = (ay.min(by) - rad - 1.0).floor() as i32;
    let y1 = (ay.max(by) + rad + 1.0).ceil() as i32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let d = seg_dist(x as f32 + 0.5, y as f32 + 0.5, ax, ay, bx, by);
            if d <= rad {
                let s = clamp01(0.75 - d / rad.max(0.001) * 0.5);
                cv.put(x, y, mix(bot, top, s));
            }
        }
    }
}

fn tri(cv: &mut Canvas, p: [(f32, f32); 3], c: Rgb, edge: Rgb) {
    let xs = [p[0].0, p[1].0, p[2].0];
    let ys = [p[0].1, p[1].1, p[2].1];
    let x0 = xs.iter().cloned().fold(f32::MAX, f32::min).floor() as i32 - 1;
    let x1 = xs.iter().cloned().fold(f32::MIN, f32::max).ceil() as i32 + 1;
    let y0 = ys.iter().cloned().fold(f32::MAX, f32::min).floor() as i32 - 1;
    let y1 = ys.iter().cloned().fold(f32::MIN, f32::max).ceil() as i32 + 1;
    let ar = |ax: f32, ay: f32, bx: f32, by: f32, cx: f32, cy: f32| (bx - ax) * (cy - ay) - (by - ay) * (cx - ax);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let d1 = ar(p[0].0, p[0].1, p[1].0, p[1].1, fx, fy);
            let d2 = ar(p[1].0, p[1].1, p[2].0, p[2].1, fx, fy);
            let d3 = ar(p[2].0, p[2].1, p[0].0, p[0].1, fx, fy);
            if !((d1 < 0.0 || d2 < 0.0 || d3 < 0.0) && (d1 > 0.0 || d2 > 0.0 || d3 > 0.0)) {
                let m = d1.abs().min(d2.abs()).min(d3.abs());
                cv.put(x, y, if m < 1.1 { edge } else { c });
            }
        }
    }
}

// ---------------- creature spec ----------------
#[derive(Clone, Copy, PartialEq)]
enum Mouth {
    Beak,
    Hook,
    Tube,
    Mandible,
    Maw,
}
#[derive(Clone, Copy, PartialEq)]
enum Tail {
    None,
    Tuft,
    Fork,
    Fan,
    Spike,
    Whip,
}
#[derive(Clone, Copy, PartialEq)]
enum Gear {
    None,
    Antennae,
    Horns,
    Crest,
    Frill,
}
/// Body plan = the SKELETON archetype. These are BLEND ENDPOINTS: a genus picks
/// two of them + a weight, so most creatures are hybrids (ostrich×macaw,
/// gull×alligator, ...) rather than exact replicas.
#[derive(Clone, Copy, PartialEq)]
enum Plan {
    Perched,     // small rounded bird
    Ratite,      // ostrich/emu: tiny head, towering neck, long thick legs
    Upright,     // penguin: vertical torpedo, short legs
    Plump,       // dodo: fat body, big hooked head, stubby legs
    LongNeck,    // swan/goose: low body, long curved neck
    Waterfowl,   // duck/gull: low horizontal body, broad bill
    Wader,       // heron/flamingo: long stilt legs, long neck+beak
    Display,     // peacock: huge fan train + crest
    Parrot,      // macaw: hooked beak, crest, very long tail
    Reptile,     // alligator: long low body, toothy maw, spiky back, wide stance
    Pelican,     // huge deep bill
    Hummingbird, // tiny body, long needle bill
    Raptor,      // hawk: hooked beak, broad fan tail, bulky
    Serpent,     // long S-neck, tiny body, whip tail, no legs
    Bat,         // small body, big membrane wing, hooked face, ear-crest
    Abyssal,     // anglerfish: tiny body, huge toothy maw, lure-crest
    Insectoid,   // fuzzy body, proboscis, 3 legs, wings, antenna-crest
    Titan,       // sauropod: huge body, towering neck, tiny head, whip tail
    Crustacean,  // wide armored body, mandibles, 3 legs, spiky back
    Amphibian,   // wide body, gaping maw, big frill-crest, stubby legs
}
const PLANS: [Plan; 20] = [
    Plan::Perched,
    Plan::Ratite,
    Plan::Upright,
    Plan::Plump,
    Plan::LongNeck,
    Plan::Waterfowl,
    Plan::Wader,
    Plan::Display,
    Plan::Parrot,
    Plan::Reptile,
    Plan::Pelican,
    Plan::Hummingbird,
    Plan::Raptor,
    Plan::Serpent,
    Plan::Bat,
    Plan::Abyssal,
    Plan::Insectoid,
    Plan::Titan,
    Plan::Crustacean,
    Plan::Amphibian,
];

struct Alien {
    body: Ell,
    head: Ell,
    neck: Option<(f32, f32, f32, f32, f32)>,
    neck_curve: f32,
    mouth: Mouth,
    mouth_len: f32,
    mouth_half: f32,
    eyes: u32,
    eyes_vertical: bool,
    eye_r: f32,
    gear: Gear,
    gear_n: u32,
    dorsal: bool,
    tail: Tail,
    tail_len: f32,
    tail_half: f32,
    legs: u32,
    tentacle: bool,
    leg_len: f32,
    leg_thick: f32,
    leg_spread: f32,
    wing_stub: bool,
    wing_membrane: bool,
    eyestalk: bool,
    lure: bool,
    pat: Pattern,
    // colors
    back: Rgb,
    belly: Rgb,
    wing_col: Rgb,
    patc: Rgb,
    beak_col: Rgb,
    leg_col: Rgb,
    accent: Rgb,
    iris: Rgb,
    outline: Rgb,
    hue: f32,
    // animation amplitudes (per-creature, seeded; scaled by genus energy)
    bob_amp: f32,
    blink_frame: i32,
    sway_amp: f32,
    wag_amp: f32,
    step_amp: f32,
    flap_amp: f32,
    anim_tempo: u32,
    // appendage randomization (per-individual)
    wing_span: f32,
    wing_angle: f32,
    wing_struts: u32,
    stalk_len: f32,
    stalk_splay: f32,
    lure_len: f32,
    lure_arc: f32,
    horn_len: f32,
    horn_angle: f32,
    crest_n: u32,
    crest_len: f32,
    ant_len: f32,
    ant_splay: f32,
    frill_size: f32,
    pincer_len: f32,
    // appendage COUNTS
    stalk_n: u32,
    dorsal_n: u32,
    teeth_n: u32,
    fan_n: u32,
    // bioluminescence + dorsal sail
    biolum_n: u32,
    biolum_spots: [(f32, f32); 8],
    biolum_col: Rgb,
    fin: bool,
    fin_h: f32,
    fin_rays: u32,
}

/// A GENUS is a family template: it fixes a color family, a body plan, and a
/// set of *signature* features. Each genus is itself randomly extreme, so
/// different planets get wildly different fauna. `mutation` is how often an
/// individual ignores the template and rerolls a trait fully random — that's
/// what keeps the bizarre oddballs alive inside a coherent family.
struct Genus {
    plan_a: Plan,
    plan_b: Plan,
    blend: f32,
    hue_c: f32,
    hue_spread: f32,
    sat: (f32, f32),
    val: (f32, f32),
    scheme: u32,
    gear: Gear,
    pat: Pattern,
    eyes: u32,
    eyes_vertical_p: f32,
    eye_r: (f32, f32),
    tentacle_p: f32,
    mutation: f32,
    // movement personality (shared by the family)
    anim_energy: f32,
    anim_tempo: u32,
}

fn sample_genus(rng: &mut StdRng) -> Genus {
    let el = r(rng, 1.5, 3.0);
    // pick two DISTINCT plan endpoints to hybridize
    let ia = rng.gen_range(0..PLANS.len());
    let ib = (ia + rng.gen_range(1..PLANS.len())) % PLANS.len();
    Genus {
        plan_a: PLANS[ia],
        plan_b: PLANS[ib],
        // bias toward the extremes: one archetype dominates, the other twists it
        blend: if rng.gen_bool(0.5) { r(rng, 0.12, 0.35) } else { r(rng, 0.65, 0.88) },
        hue_c: rng.gen::<f32>(),
        hue_spread: r(rng, 0.02, 0.10),
        sat: (r(rng, 0.5, 0.7), r(rng, 0.75, 0.98)),
        val: (r(rng, 0.45, 0.6), r(rng, 0.7, 0.85)),
        scheme: rng.gen_range(0..3),
        gear: [Gear::None, Gear::Antennae, Gear::Horns, Gear::Crest, Gear::Frill][rng.gen_range(0..5)],
        pat: [Pattern::Solid, Pattern::Solid, Pattern::Spots, Pattern::Stripes, Pattern::Dorsal, Pattern::Irid][rng.gen_range(0..6)],
        eyes: [1u32, 2, 2, 3][rng.gen_range(0..4)],
        eyes_vertical_p: if rng.gen_bool(0.35) { 0.8 } else { 0.1 },
        eye_r: (1.0 + el * 0.2, 1.2 + el * 0.5),
        tentacle_p: if rng.gen_bool(0.3) { 0.85 } else { 0.1 },
        mutation: r(rng, 0.12, 0.28),
        anim_energy: r(rng, 0.5, 1.5), // languid <-> twitchy family
        anim_tempo: if rng.gen_bool(0.3) { 2 } else { 1 }, // some families move at double time
    }
}

/// The resolved skeleton for one individual (geometry only).
struct Skel {
    body: Ell,
    head: Ell,
    neck: Option<(f32, f32, f32, f32, f32)>,
    neck_curve: f32,
    legs_n: u32,
    leg_len: f32,
    leg_thick: f32,
    leg_spread: f32,
    tail_def: Tail,
    tail_len: f32,
    tail_half: f32,
    mouth_len: f32,
    mouth_half: f32,
}

/// Numeric parameters for a plan archetype. These are BLENDABLE — two of them
/// lerp into a hybrid. (All absolute px in the 30-grid; hue/features added later.)
#[derive(Clone, Copy)]
struct PP {
    body_cy: f32,
    rx: f32,
    ry: f32,
    head_r: f32,
    head_dx: f32,
    head_dy: f32,
    neck: f32,
    neck_thick: f32,
    neck_curve: f32,
    legs: f32,
    leg_len: f32,
    leg_thick: f32,
    leg_spread: f32,
    tail_len: f32,
    tail_half: f32,
    tail_kind: Tail,
    mouth_len: f32,
    mouth_half: f32,
    mouth_kind: Mouth,
    wing: f32,
    dorsal: f32,
    crest: f32,
}

#[allow(clippy::too_many_arguments)]
fn mk(
    body_cy: f32, rx: f32, ry: f32, head_r: f32, head_dx: f32, head_dy: f32,
    neck: f32, neck_thick: f32, neck_curve: f32, legs: f32, leg_len: f32, leg_thick: f32, leg_spread: f32,
    tail_len: f32, tail_half: f32, tail_kind: Tail, mouth_len: f32, mouth_half: f32, mouth_kind: Mouth,
    wing: f32, dorsal: f32, crest: f32,
) -> PP {
    PP { body_cy, rx, ry, head_r, head_dx, head_dy, neck, neck_thick, neck_curve, legs, leg_len, leg_thick, leg_spread, tail_len, tail_half, tail_kind, mouth_len, mouth_half, mouth_kind, wing, dorsal, crest }
}

fn params(plan: Plan) -> PP {
    use Mouth::*;
    use Tail::*;
    //         cy    rx   ry  hr  hdx   hdy  neck nthk ncur legs ll   lt   ls   tl    th  tail    ml   mh  mouth   wing dors crest
    match plan {
        Plan::Perched     => mk(19.0, 5.7, 5.7, 4.0, 4.0, -4.0, 0.0, 1.6, 0.0, 2.0, 3.7, 0.7, 2.9, 6.5, 2.2, Tuft,  4.0, 1.6, Beak,   0.6, 0.2, 0.1),
        Plan::Ratite      => mk(13.5, 6.7, 6.1, 2.6, 3.0, -8.5, 1.0, 1.9, 0.8, 2.0, 9.2, 1.35,2.3, 4.0, 2.0, Tuft,  2.5, 1.3, Beak,   0.15,0.15,0.15),
        Plan::Upright     => mk(18.0, 5.0, 9.0, 3.9, 1.0, -7.4, 0.0, 1.6, 0.0, 2.0, 2.5, 1.0, 2.7, 3.5, 1.5, Spike, 3.5, 1.4, Beak,   0.85,0.1, 0.1),
        Plan::Plump       => mk(20.0, 8.7, 7.0, 5.0, 5.2, -6.0, 0.7, 2.8, 0.4, 2.0, 3.2, 1.2, 3.5, 3.7, 2.0, Tuft,  4.7, 2.5, Hook,   0.3, 0.1, 0.15),
        Plan::LongNeck    => mk(21.0, 7.7, 5.5, 3.3, 4.0,-13.0, 1.0, 1.9, 3.0, 1.4, 3.5, 0.8, 3.0, 5.0, 2.0, Tuft,  3.7, 1.8, Beak,   0.4, 0.1, 0.1),
        Plan::Waterfowl   => mk(21.0, 8.5, 5.8, 3.9, 5.5, -4.6, 0.0, 2.2, 0.0, 2.0, 3.0, 0.9, 3.4, 5.0, 2.0, Tuft,  4.5, 2.5, Tube,   0.6, 0.1, 0.1),
        Plan::Wader       => mk(14.5, 6.5, 4.8, 2.9, 3.3, -8.0, 1.0, 1.5, 2.0, 2.0,10.5, 0.65,2.0, 4.0, 1.8, Tuft,  6.0, 1.2, Tube,   0.4, 0.1, 0.2),
        Plan::Display     => mk(19.0, 6.5, 6.5, 3.3, 3.9, -6.0, 0.7, 1.6, 0.3, 2.0, 5.0, 0.8, 3.2,12.5, 7.5, Fan,   3.5, 1.5, Beak,   0.5, 0.1, 0.9),
        Plan::Parrot      => mk(19.0, 6.0, 6.5, 4.2, 4.5, -5.0, 0.3, 2.0, 0.3, 2.0, 3.0, 0.9, 2.5,13.0, 1.6, Whip,  3.5, 2.6, Hook,   0.6, 0.1, 0.7),
        Plan::Reptile     => mk(21.0, 9.0, 5.0, 3.8, 6.5, -2.0, 0.2, 2.2, 0.0, 2.0, 2.8, 1.2, 4.8,11.0, 2.2, Spike, 6.5, 2.6, Maw,    0.05,0.95,0.1),
        Plan::Pelican     => mk(20.0, 8.0, 6.0, 4.0, 5.5, -5.5, 0.5, 2.4, 0.5, 2.0, 3.0, 0.9, 3.0, 4.0, 2.0, Tuft,  7.5, 3.2, Tube,   0.5, 0.1, 0.1),
        Plan::Hummingbird => mk(18.0, 4.2, 4.0, 3.0, 3.2, -3.5, 0.0, 1.2, 0.0, 2.0, 2.0, 0.5, 1.6, 4.0, 1.5, Fork,  7.5, 0.8, Tube,   0.95,0.05,0.2),
        Plan::Raptor      => mk(18.0, 7.0, 7.0, 4.3, 4.5, -6.0, 0.3, 2.2, 0.2, 2.0, 4.0, 1.1, 3.0, 7.0, 3.0, Fan,   4.0, 2.2, Hook,   0.6, 0.1, 0.1),
        Plan::Serpent     => mk(22.0, 5.3, 4.3, 3.0, 3.0,-11.0, 1.0, 1.6, 4.0, 0.0, 0.0, 0.0, 0.0, 9.0, 1.5, Whip,  4.0, 1.6, Maw,    0.1, 0.4, 0.1),
        Plan::Bat         => mk(18.0, 5.0, 5.0, 3.6, 3.5, -4.0, 0.0, 1.5, 0.0, 2.0, 2.5, 0.6, 2.0, 6.0, 1.2, Whip,  3.0, 2.0, Hook,   0.95,0.1, 0.7),
        Plan::Abyssal     => mk(20.0, 6.5, 6.0, 4.5, 4.5, -2.5, 0.0, 1.8, 0.0, 0.0, 0.0, 0.0, 0.0, 7.0, 1.5, Whip,  6.0, 3.2, Maw,    0.1, 0.3, 0.55),
        Plan::Insectoid   => mk(19.0, 5.5, 5.5, 3.6, 3.8, -4.0, 0.2, 1.6, 0.0, 3.0, 3.0, 0.6, 3.2, 4.0, 2.0, Fork,  6.0, 0.9, Tube,   0.85,0.2, 0.6),
        Plan::Titan       => mk(20.0, 9.5, 7.0, 2.4, 5.0,-13.0, 1.0, 2.6, 2.0, 2.0, 5.0, 1.8, 4.5,12.0, 2.5, Whip,  3.0, 1.4, Beak,   0.05,0.3, 0.1),
        Plan::Crustacean  => mk(21.0, 8.0, 5.5, 3.5, 5.5, -2.5, 0.0, 2.0, 0.0, 3.0, 3.5, 1.0, 5.0, 5.0, 2.0, Spike, 5.0, 2.4, Mandible, 0.05,0.8, 0.4),
        Plan::Amphibian   => mk(21.0, 7.5, 6.0, 4.5, 5.0, -3.5, 0.2, 2.2, 0.0, 2.0, 2.5, 1.1, 3.5, 4.0, 2.0, Tuft,  5.0, 2.6, Maw,    0.1, 0.2, 0.85),
    }
}

/// Bespoke appendage biases per plan: (membrane wing, eyestalks, lure, glow, fin).
/// Blended between the genus's two endpoints just like the numeric params.
fn appendages(plan: Plan) -> (f32, f32, f32, f32, f32) {
    match plan {
        //                        wing  stalk lure  glow  fin
        Plan::Bat => (1.0, 0.0, 0.0, 0.0, 0.0),
        Plan::Insectoid => (0.85, 0.0, 0.0, 0.45, 0.0),
        Plan::Crustacean => (0.0, 1.0, 0.0, 0.0, 0.3),
        Plan::Abyssal => (0.0, 0.55, 1.0, 0.85, 0.35),
        Plan::Reptile => (0.0, 0.0, 0.0, 0.0, 0.55),
        Plan::Amphibian => (0.0, 0.0, 0.0, 0.35, 0.5),
        Plan::Serpent => (0.0, 0.0, 0.0, 0.25, 0.4),
        _ => (0.0, 0.0, 0.0, 0.0, 0.0),
    }
}

fn blend_pp(a: &PP, b: &PP, t: f32) -> PP {
    let l = |x: f32, y: f32| x + (y - x) * t;
    PP {
        body_cy: l(a.body_cy, b.body_cy),
        rx: l(a.rx, b.rx),
        ry: l(a.ry, b.ry),
        head_r: l(a.head_r, b.head_r),
        head_dx: l(a.head_dx, b.head_dx),
        head_dy: l(a.head_dy, b.head_dy),
        neck: l(a.neck, b.neck),
        neck_thick: l(a.neck_thick, b.neck_thick),
        neck_curve: l(a.neck_curve, b.neck_curve),
        legs: l(a.legs, b.legs),
        leg_len: l(a.leg_len, b.leg_len),
        leg_thick: l(a.leg_thick, b.leg_thick),
        leg_spread: l(a.leg_spread, b.leg_spread),
        tail_len: l(a.tail_len, b.tail_len),
        tail_half: l(a.tail_half, b.tail_half),
        tail_kind: if t < 0.5 { a.tail_kind } else { b.tail_kind },
        mouth_len: l(a.mouth_len, b.mouth_len),
        mouth_half: l(a.mouth_half, b.mouth_half),
        mouth_kind: if t < 0.5 { a.mouth_kind } else { b.mouth_kind },
        wing: l(a.wing, b.wing),
        dorsal: l(a.dorsal, b.dorsal),
        crest: l(a.crest, b.crest),
    }
}

fn skeleton(pp: &PP, rng: &mut StdRng) -> Skel {
    let cx = 13.5;
    let body = Ell { cx, cy: pp.body_cy + r(rng, -0.5, 0.5), rx: pp.rx * r(rng, 0.92, 1.08), ry: pp.ry * r(rng, 0.92, 1.08) };
    let head = Ell {
        cx: (body.cx + pp.head_dx).min(25.0),
        cy: (body.cy + pp.head_dy).max(3.5),
        rx: pp.head_r,
        ry: pp.head_r * r(rng, 0.9, 1.05),
    };
    let neck = if pp.neck > 0.4 {
        Some((body.cx + body.rx * 0.3, body.cy - body.ry * 0.6, head.cx, head.cy, pp.neck_thick))
    } else {
        None
    };
    let legs_n = if pp.legs < 1.0 { 0 } else if pp.legs < 2.5 { 2 } else { 3 };
    Skel {
        body,
        head,
        neck,
        neck_curve: pp.neck_curve,
        legs_n,
        leg_len: pp.leg_len,
        leg_thick: pp.leg_thick,
        leg_spread: pp.leg_spread,
        tail_def: pp.tail_kind,
        tail_len: pp.tail_len,
        tail_half: pp.tail_half,
        mouth_len: pp.mouth_len,
        mouth_half: pp.mouth_half,
    }
}

fn sample(g: &Genus, rng: &mut StdRng) -> Alien {
    let m = g.mutation;
    // categorical: inherit the genus trait, or (with prob m) reroll fully random
    macro_rules! mutate {
        ($genus:expr, $rand:expr) => {
            if rng.gen::<f32>() < m { $rand } else { $genus }
        };
    }

    // --- geometry: blend the genus's two plan endpoints, then build skeleton ---
    let pp = blend_pp(&params(g.plan_a), &params(g.plan_b), g.blend);
    let sk = skeleton(&pp, rng);

    // --- bespoke appendages (blended per-plan) ---
    let (ma, ea, la, ga, fa) = appendages(g.plan_a);
    let (mb, eb, lb, gb, fb) = appendages(g.plan_b);
    let bl = g.blend;
    let lp = |x: f32, y: f32| x + (y - x) * bl;
    let wing_membrane = lp(ma, mb) > 0.45;
    let eyestalk = lp(ea, eb) > 0.5;
    let lure = lp(la, lb) > 0.5;
    let biolum = lp(ga, gb) > 0.4 || rng.gen_bool(0.12);
    let fin = lp(fa, fb) > 0.4;

    // bioluminescent spots scattered on the body (seeded positions)
    let biolum_n = if biolum { rng.gen_range(3..=7) } else { 0 };
    let mut biolum_spots = [(0.0f32, 0.0f32); 8];
    for spot in biolum_spots.iter_mut().take(biolum_n as usize) {
        let ang = r(rng, 0.0, TAU);
        let rad = r(rng, 0.15, 0.85);
        *spot = (sk.body.cx + ang.cos() * rad * sk.body.rx * 0.8, sk.body.cy + ang.sin() * rad * sk.body.ry * 0.8);
    }
    let biolum_col = if rng.gen_bool(0.5) { hsv((rng.gen::<f32>()).fract(), 0.75, 1.0) } else { [0.5, 1.0, 0.9] };

    // --- features (plan biases + genus signature + mutation) ---
    let mouth = mutate!(pp.mouth_kind, [Mouth::Beak, Mouth::Hook, Mouth::Tube, Mouth::Mandible, Mouth::Maw][rng.gen_range(0..5)]);
    let eyes = mutate!(g.eyes, [1u32, 2, 3][rng.gen_range(0..3)]);
    let mut gear = mutate!(g.gear, [Gear::None, Gear::Antennae, Gear::Horns, Gear::Crest, Gear::Frill][rng.gen_range(0..5)]);
    if pp.crest > 0.6 && rng.gen_bool(0.7) {
        gear = Gear::Crest; // strongly-crested hybrids (macaw/peacock lineage)
    }
    let tail = mutate!(sk.tail_def, [Tail::None, Tail::Tuft, Tail::Fork, Tail::Fan, Tail::Spike, Tail::Whip][rng.gen_range(0..6)]);
    let pat = mutate!(g.pat, [Pattern::Solid, Pattern::Spots, Pattern::Stripes, Pattern::Dorsal, Pattern::Irid][rng.gen_range(0..5)]);

    // --- palette (genus color family) ---
    let hue = (g.hue_c + r(rng, -g.hue_spread, g.hue_spread)).rem_euclid(1.0);
    let h2u = match g.scheme {
        0 => (hue + 0.5).fract(),
        1 => (hue + 0.33).fract(),
        _ => (hue + 0.08).fract(),
    };
    let back = hsv(hue, r(rng, g.sat.0, g.sat.1), r(rng, g.val.0, g.val.1));
    let belly = if rng.gen_bool(0.5) { lighter(back, 0.4) } else { hsv(h2u, r(rng, 0.4, 0.8), r(rng, 0.7, 0.95)) };
    let wing_col = darker(hsv(h2u, 0.7, 0.7), 0.15);
    let patc = hsv((hue + r(rng, 0.4, 0.6)).fract(), 0.75, r(rng, 0.35, 0.7));
    let accent = hsv((hue + 0.5).fract(), 0.9, 0.95);
    let iris = if rng.gen_bool(0.5) { hsv((hue + 0.5).fract(), 0.9, 1.0) } else { [0.98, 0.85, 0.25] };
    let beak_col = *[hsv((hue + 0.5).fract(), 0.8, 0.9), [0.9, 0.85, 0.7], [0.2, 0.18, 0.22]].get(rng.gen_range(0..3)).unwrap();
    let leg_col = if rng.gen_bool(0.5) { darker(back, 0.35) } else { hsv((hue + 0.5).fract(), 0.7, 0.7) };

    let wing_stub = mutate!(rng.gen::<f32>() < clamp01(pp.wing), rng.gen_bool(0.5));
    let dorsal = rng.gen::<f32>() < clamp01(pp.dorsal + 0.12);

    Alien {
        body: sk.body,
        head: sk.head,
        neck: sk.neck,
        neck_curve: sk.neck_curve,
        mouth,
        mouth_len: sk.mouth_len,
        mouth_half: sk.mouth_half,
        eyes,
        eyes_vertical: rng.gen::<f32>() < g.eyes_vertical_p,
        eye_r: r(rng, g.eye_r.0, g.eye_r.1),
        gear,
        gear_n: rng.gen_range(1..=3),
        dorsal,
        tail,
        tail_len: sk.tail_len,
        tail_half: sk.tail_half,
        legs: sk.legs_n,
        tentacle: rng.gen::<f32>() < g.tentacle_p,
        leg_len: sk.leg_len,
        leg_thick: sk.leg_thick,
        leg_spread: sk.leg_spread,
        wing_stub,
        wing_membrane,
        eyestalk,
        lure,
        pat,
        back,
        belly,
        wing_col,
        patc,
        beak_col,
        leg_col,
        accent,
        iris,
        outline: darker(back, 0.82),
        hue,
        bob_amp: r(rng, 0.6, 1.6) * g.anim_energy,
        blink_frame: if rng.gen_bool(0.6) { rng.gen_range(0..ANIM_FRAMES) as i32 } else { -1 },
        sway_amp: r(rng, 0.6, 1.8) * g.anim_energy,
        wag_amp: r(rng, 0.8, 2.4) * g.anim_energy,
        step_amp: r(rng, 0.4, 1.3) * g.anim_energy,
        flap_amp: r(rng, 0.15, 0.55) * g.anim_energy,
        anim_tempo: g.anim_tempo,
        wing_span: r(rng, 0.8, 1.45),
        wing_angle: r(rng, -0.35, 0.6),
        wing_struts: rng.gen_range(2..=4),
        stalk_len: r(rng, 3.0, 7.5),
        stalk_splay: r(rng, 0.2, 0.55),
        lure_len: r(rng, 0.85, 1.6),
        lure_arc: r(rng, 2.5, 6.5),
        horn_len: r(rng, 2.5, 4.8),
        horn_angle: r(rng, -0.5, 0.5),
        crest_n: rng.gen_range(3..=7),
        crest_len: r(rng, 1.8, 3.6),
        ant_len: r(rng, 3.5, 7.5),
        ant_splay: r(rng, 1.8, 4.5),
        frill_size: r(rng, 0.8, 1.4),
        pincer_len: r(rng, 1.5, 3.2),
        stalk_n: rng.gen_range(1..=3),
        dorsal_n: rng.gen_range(3..=8),
        teeth_n: rng.gen_range(2..=5),
        fan_n: rng.gen_range(3..=7),
        biolum_n,
        biolum_spots,
        biolum_col,
        fin,
        fin_h: r(rng, 3.0, 7.0),
        fin_rays: rng.gen_range(4..=8),
    }
}

/// A curved neck: quadratic bezier from A to B, control point offset
/// perpendicular by `curve` (0 = straight). Drawn as chained capsules.
fn curved_neck(cv: &mut Canvas, ax: f32, ay: f32, bx: f32, by: f32, thick: f32, curve: f32, top: Rgb, bot: Rgb) {
    let (mx, my) = ((ax + bx) / 2.0, (ay + by) / 2.0);
    let (dx, dy) = (bx - ax, by - ay);
    let len = (dx * dx + dy * dy).sqrt().max(0.001);
    let (px, py) = (-dy / len, dx / len);
    let (cxp, cyp) = (mx + px * curve, my + py * curve);
    let steps = 12;
    let mut prev = (ax, ay);
    for i in 1..=steps {
        let t = i as f32 / steps as f32;
        let it = 1.0 - t;
        let x = it * it * ax + 2.0 * it * t * cxp + t * t * bx;
        let y = it * it * ay + 2.0 * it * t * cyp + t * t * by;
        capsule(cv, prev.0, prev.1, x, y, thick, top, bot);
        prev = (x, y);
    }
}

/// Legs drawn INTO the silhouette (so they're thick + outlined), with the
/// idle step offset. Straight legs, or wavy tentacles.
fn draw_legs(cv: &mut Canvas, a: &Alien, s: f32) {
    if a.legs == 0 {
        return;
    }
    let top = a.body.cy + a.body.ry * 0.35; // start inside the body -> attached
    let foot = top + a.leg_len;
    for i in 0..a.legs {
        let t = if a.legs == 1 { 0.5 } else { i as f32 / (a.legs as f32 - 1.0) };
        let base_x = a.body.cx - a.leg_spread * 0.5 + t * a.leg_spread;
        let step = if i % 2 == 0 { 1.0 } else { -1.0 } * a.step_amp * s;
        let ctop = a.leg_col;
        let cbot = darker(a.leg_col, 0.3);
        if a.tentacle {
            let seg = 6;
            let mut prev = (base_x, top);
            for k in 1..=seg {
                let tt = k as f32 / seg as f32;
                let x = base_x + (tt * 6.0 + s).sin() * 2.0 + step * tt;
                let y = top + (foot - top) * tt;
                capsule(cv, prev.0, prev.1, x, y, a.leg_thick, ctop, cbot);
                prev = (x, y);
            }
            capsule(cv, prev.0, prev.1, prev.0 + 2.0, prev.1, a.leg_thick, ctop, cbot);
        } else {
            let footx = base_x + step;
            capsule(cv, base_x, top, footx, foot, a.leg_thick, ctop, cbot);
            capsule(cv, footx, foot, footx + 2.5, foot, a.leg_thick * 0.9, ctop, cbot); // foot
        }
    }
}

/// A bat/insect membrane wing: a fan of triangles from the shoulder with darker
/// finger-struts along the leading edges. Flaps (raises/lowers) with `s`.
fn draw_membrane_wing(cv: &mut Canvas, a: &Alien, s: f32) {
    let flap = a.flap_amp * s * 3.5;
    let span = a.wing_span;
    let (c, si) = (a.wing_angle.cos(), a.wing_angle.sin());
    let sh = (a.body.cx + a.body.rx * 0.15, a.body.cy - a.body.ry * 0.45); // shoulder
    // rotate the tip/elbow offsets by wing_angle for a randomized sweep
    let rot = |ox: f32, oy: f32| (sh.0 + ox * c - oy * si, sh.1 + ox * si + oy * c);
    let mut tip = rot(-a.body.rx * 1.3 * span, -a.body.ry * 1.7 * span);
    tip.1 -= flap;
    let mut elbow = rot(-a.body.rx * 0.6 * span, -a.body.ry * 1.0 * span);
    elbow.1 -= flap * 0.5;
    let mid = (a.body.cx - a.body.rx * 0.75 * span, a.body.cy + a.body.ry * 0.05);
    let low = (a.body.cx - a.body.rx * 0.2, a.body.cy + a.body.ry * 0.25);
    let mem = a.wing_col;
    let edge = darker(a.wing_col, 0.35);
    tri(cv, [sh, tip, elbow], mem, edge);
    tri(cv, [sh, elbow, mid], mem, edge);
    tri(cv, [sh, mid, low], mem, edge);
    // finger struts — count varies per creature
    let strut = darker(a.wing_col, 0.55);
    let leading = [tip, elbow, mid];
    for k in 0..(a.wing_struts as usize).min(3) {
        capsule(cv, sh.0, sh.1, leading[k].0, leading[k].1, 0.7, strut, strut);
    }
    if a.wing_struts >= 4 {
        let extra = ((tip.0 + elbow.0) / 2.0, (tip.1 + elbow.1) / 2.0);
        capsule(cv, sh.0, sh.1, extra.0, extra.1, 0.7, strut, strut);
    }
}

/// A dorsal sail: a webbed membrane along the back with ray struts, tallest in
/// the middle. Drawn before the body so the body covers its base (attached).
fn draw_sail(cv: &mut Canvas, a: &Alien, s: f32) {
    let n = a.fin_rays.max(3);
    let x0 = a.body.cx - a.body.rx * 0.65;
    let x1 = a.body.cx + a.body.rx * 0.45;
    let mem = a.wing_col;
    let edge = darker(a.wing_col, 0.3);
    let ray = darker(a.accent, 0.1);
    let mut prev: Option<((f32, f32), (f32, f32))> = None;
    for i in 0..n {
        let t = i as f32 / (n as f32 - 1.0);
        let bx = x0 + (x1 - x0) * t;
        let nx = (bx - a.body.cx) / a.body.rx;
        let bytop = a.body.cy - a.body.ry * (1.0 - nx * nx).max(0.0).sqrt();
        let h = a.fin_h * (std::f32::consts::PI * t).sin();
        let ripple = (t * 5.0).sin() * s * 1.0; // membrane shimmer
        let base = (bx, bytop);
        let tip = (bx + ripple, bytop - h);
        if let Some((pb, pt)) = prev {
            tri(cv, [pb, pt, tip], mem, edge);
            tri(cv, [pb, tip, base], mem, edge);
        }
        capsule(cv, base.0, base.1, tip.0, tip.1, 0.6, ray, darker(a.accent, 0.4));
        prev = Some((base, tip));
    }
}

fn draw(a: &Alien, s: f32, s2: f32, blink: bool) -> RgbaImage {
    let mut cv = Canvas::new();

    // membrane wing goes behind the body (spreads up and back)
    if a.wing_membrane {
        draw_membrane_wing(&mut cv, a, s);
    }

    // legs first, into the silhouette (body will cover their tops -> attached)
    draw_legs(&mut cv, a, s);

    // tail (behind) — wags with `s`
    {
        let bx = a.body.cx - a.body.rx * 0.85;
        let by = a.body.cy;
        let wag = a.wag_amp * s;
        let tip = (bx - a.tail_len, by - a.tail_len * 0.25 + wag);
        match a.tail {
            Tail::None => {}
            Tail::Tuft => tri(&mut cv, [(bx, by - a.tail_half), (bx, by + a.tail_half), (bx - a.tail_len * 0.6, by + wag)], a.wing_col, darker(a.wing_col, 0.3)),
            Tail::Fork => {
                tri(&mut cv, [(bx, by - 2.0), (bx, by), (tip.0, tip.1 - 2.0)], a.wing_col, darker(a.wing_col, 0.3));
                tri(&mut cv, [(bx, by), (bx, by + 2.0), (tip.0, tip.1 + 2.0)], a.wing_col, darker(a.wing_col, 0.3));
            }
            Tail::Fan => {
                tri(&mut cv, [(bx, by - a.tail_half), (bx, by + a.tail_half), (bx - a.tail_len, by + wag)], a.accent, darker(a.accent, 0.3));
                // train feathers
                let n = a.fan_n;
                for k in 0..=n {
                    let ty = by - a.tail_half + (2.0 * a.tail_half) * (k as f32 / n as f32);
                    capsule(&mut cv, bx - 1.0, ty, bx - a.tail_len, by + wag + (ty - by) * 0.3, 0.7, darker(a.accent, 0.15), darker(a.accent, 0.4));
                }
            }
            Tail::Spike => tri(&mut cv, [(bx, by - 1.5), (bx, by + 1.5), tip], darker(a.back, 0.2), a.outline),
            Tail::Whip => {
                // undulating, tapering whip
                let seg = 6;
                let mut prev = (bx, by);
                for k in 1..=seg {
                    let t = k as f32 / seg as f32;
                    let x = bx - a.tail_len * t;
                    let y = by - a.tail_len * 0.25 * t + wag + (t * 4.0).sin() * s * 1.6;
                    capsule(&mut cv, prev.0, prev.1, x, y, 1.1 * (1.0 - t * 0.55), a.back, darker(a.back, 0.3));
                    prev = (x, y);
                }
            }
        }
    }
    // dorsal structure: a sail fin (shimmers), else spikes
    if a.fin {
        draw_sail(&mut cv, a, s);
    } else if a.dorsal {
        let n = a.dorsal_n;
        for i in 0..n {
            let t = i as f32 / (n as f32 - 1.0);
            let sx = a.body.cx - a.body.rx * 0.6 + t * a.body.rx * 1.2;
            let sy = a.body.cy - a.body.ry * (0.9 - 0.1 * (t - 0.5).abs());
            tri(&mut cv, [(sx - 1.3, sy + 1.0), (sx + 1.3, sy + 1.0), (sx, sy - 2.6)], a.accent, a.outline);
        }
    }
    // neck (curved)
    if let Some((ax, ay, nx, ny, nr)) = a.neck {
        curved_neck(&mut cv, ax, ay, nx, ny, nr, a.neck_curve, a.back, darker(a.back, 0.3));
    }
    // frill behind head
    if a.gear == Gear::Frill {
        let hx = a.head.cx;
        let hy = a.head.cy;
        for k in -2..=2 {
            let ang = k as f32 * 0.4;
            tri(
                &mut cv,
                [(hx, hy - 2.0), (hx, hy + 2.0), (hx - (5.0 * a.frill_size) * ang.cos().abs().max(0.3) - 2.0, hy + 6.0 * a.frill_size * ang.sin())],
                a.accent,
                darker(a.accent, 0.3),
            );
        }
    }
    // body (patterned) — gentle breathing on the out-of-phase signal
    let body_e = Ell { cx: a.body.cx, cy: a.body.cy, rx: a.body.rx * (1.0 + 0.02 * s2), ry: a.body.ry * (1.0 + 0.05 * s2) };
    body_blob(&mut cv, body_e, a.back, a.belly, a.pat, a.patc, a.hue);
    // wing — ON TOP of the body, at the rear flank so it's visible and reads as
    // a folded wing; protrudes past the silhouette so the outline pass frames it.
    if a.wing_stub && !a.wing_membrane {
        let flap = a.flap_amp * s; // lifts/opens with the idle loop
        let we = Ell {
            cx: a.body.cx - a.body.rx * 0.28,
            cy: a.body.cy + a.body.ry * 0.18 - flap * a.body.ry * 0.55,
            rx: a.body.rx * 0.64,
            ry: a.body.ry * 0.62 * (1.0 + flap),
        };
        blob(&mut cv, we, a.wing_col, darker(a.wing_col, 0.28));
        // covert feather lines for definition
        for k in 0..3 {
            let yy = (we.cy - 1.0 + k as f32 * 1.8) as i32;
            for xx in (we.cx - we.rx * 0.7) as i32..=(we.cx + we.rx * 0.45) as i32 {
                if cv.filledp(xx, yy) {
                    cv.put(xx, yy, darker(a.wing_col, 0.42));
                }
            }
        }
    }
    // head
    blob(&mut cv, a.head, a.back, mix(a.back, a.belly, 0.5));
    // horns / crest (into silhouette)
    match a.gear {
        Gear::Horns => {
            for i in 0..a.gear_n {
                let sx = a.head.cx - a.head.rx * 0.3 + i as f32 * a.head.rx * 0.6;
                let sy = a.head.cy - a.head.ry * 0.8;
                let tipx = sx + 1.5 + a.horn_angle * a.horn_len;
                let tipy = sy - a.horn_len;
                tri(&mut cv, [(sx - 1.4, sy + 1.0), (sx + 1.4, sy + 1.0), (tipx, tipy)], a.beak_col, a.outline);
            }
        }
        Gear::Crest => {
            for k in 0..a.crest_n {
                let sx = a.head.cx - a.head.rx * 0.6 - k as f32 * 0.6;
                let sy = a.head.cy - a.head.ry * 0.7 - k as f32 * 0.4;
                tri(&mut cv, [(sx, sy + 1.5), (sx + 1.8, sy + 1.5), (sx - 0.5, sy - a.crest_len)], a.accent, darker(a.accent, 0.3));
            }
        }
        _ => {}
    }
    // mouth (front of head)
    {
        let mx = a.head.cx + a.head.rx * 0.75;
        let my = a.head.cy + a.head.ry * 0.1;
        let up = (mx, my - a.mouth_half);
        let dn = (mx, my + a.mouth_half);
        let tipx = mx + a.mouth_len;
        match a.mouth {
            Mouth::Beak => tri(&mut cv, [up, dn, (tipx, my)], a.beak_col, darker(a.beak_col, 0.3)),
            Mouth::Hook => {
                tri(&mut cv, [up, dn, (tipx, my + 0.5)], a.beak_col, darker(a.beak_col, 0.3));
                for k in 0..2 {
                    cv.put(tipx as i32 - k, my as i32 + 1 + k, darker(a.beak_col, 0.2));
                }
            }
            Mouth::Tube => capsule(&mut cv, mx, my, tipx, my + a.mouth_len * 0.15, 1.0, a.beak_col, darker(a.beak_col, 0.3)),
            Mouth::Mandible => {
                // two curved pincers that hook inward — open/close with the loop
                let ed = darker(a.beak_col, 0.3);
                let open = s2 * 0.8;
                let ut = (tipx, my - a.mouth_half * 0.4 - open);
                tri(&mut cv, [(mx, my - a.mouth_half), (mx, my - 0.2), ut], a.beak_col, ed);
                capsule(&mut cv, ut.0, ut.1, ut.0 - a.pincer_len * 0.45, ut.1 + a.pincer_len, 0.7, a.beak_col, ed); // hook down
                let dt = (tipx, my + a.mouth_half * 0.4 + open);
                tri(&mut cv, [(mx, my + 0.2), (mx, my + a.mouth_half), dt], a.beak_col, ed);
                capsule(&mut cv, dt.0, dt.1, dt.0 - a.pincer_len * 0.45, dt.1 - a.pincer_len, 0.7, a.beak_col, ed); // hook up
            }
            Mouth::Maw => {
                tri(&mut cv, [up, dn, (tipx - a.mouth_len * 0.4, my)], a.beak_col, a.outline);
                // teeth
                for k in 0..a.teeth_n {
                    cv.put((mx + k as f32 * 1.2) as i32, (my + a.mouth_half - 0.5) as i32, [0.95, 0.95, 0.9]);
                }
            }
        }
    }

    // ---- outline pass ----
    let mut img = RgbaImage::new(GRID, GRID);
    for y in 0..GRID as i32 {
        for x in 0..GRID as i32 {
            if cv.filledp(x, y) {
                img.put_pixel(x as u32, y as u32, to_rgba(cv.col[(y as u32 * GRID + x as u32) as usize]));
            } else if (-1..=1).any(|dy| (-1..=1).any(|dx| (dx != 0 || dy != 0) && cv.filledp(x + dx, y + dy))) {
                img.put_pixel(x as u32, y as u32, to_rgba(a.outline));
            }
        }
    }

    // ---- thin details on top (bounds-checked) ----
    let mut sp = |x: i32, y: i32, c: Rgb| {
        if x >= 0 && y >= 0 && (x as u32) < GRID && (y as u32) < GRID {
            img.put_pixel(x as u32, y as u32, to_rgba(c));
        }
    };
    // (legs are now drawn into the silhouette above)
    // antennae
    if a.gear == Gear::Antennae {
        for i in 0..a.gear_n {
            let sx = a.head.cx - 1.0 + i as f32 * 2.0;
            let sy = a.head.cy - a.head.ry * 0.8;
            let tx = sx + (i as f32 - 0.5) * a.ant_splay + a.sway_amp * s; // antennae sway
            let ty = sy - a.ant_len;
            let steps = 6;
            for s in 0..=steps {
                let t = s as f32 / steps as f32;
                sp((sx + (tx - sx) * t) as i32, (sy + (ty - sy) * t) as i32, a.outline);
            }
            sp(tx as i32, ty as i32, a.accent);
            sp(tx as i32 + 1, ty as i32, a.accent);
            sp(tx as i32, ty as i32 - 1, lighter(a.accent, 0.3));
        }
    }
    // eyes — on bespoke stalks (crustacean/abyssal), else fitted to the head.
    if a.eyestalk {
        for i in 0..a.stalk_n {
            let off = i as f32 - (a.stalk_n as f32 - 1.0) / 2.0;
            let bx = a.head.cx + off * a.head.rx * a.stalk_splay * 1.3;
            let by = a.head.cy - a.head.ry * 0.55;
            let tx = bx + off * (a.stalk_splay * 4.5) + a.sway_amp * s * 0.4;
            let ty = by - a.stalk_len + s2 * 0.7; // stalks bob
            for k in 0..=5 {
                let t = k as f32 / 5.0;
                sp((bx + (tx - bx) * t) as i32, (by + (ty - by) * t) as i32, a.outline);
            }
            for dy in -1..=1 {
                for dx in -1..=1 {
                    if dx * dx + dy * dy <= 2 {
                        sp(tx as i32 + dx, ty as i32 + dy, a.iris);
                    }
                }
            }
            sp(tx as i32, ty as i32, [0.05, 0.04, 0.08]);
            sp(tx as i32, ty as i32 - 1, [1.0, 1.0, 1.0]);
        }
    } else {
        let rr = a.eye_r.min(a.head.rx * if a.eyes >= 2 { 0.40 } else { 0.55 });
        let axis = if a.eyes_vertical { a.head.ry } else { a.head.rx };
        let mut spacing = rr * 2.0 + 0.5;
        let limit = axis * 1.4;
        if a.eyes > 1 && (a.eyes as f32 - 1.0) * spacing > limit {
            spacing = limit / (a.eyes as f32 - 1.0);
        }
        let cxp = a.head.cx + a.head.rx * 0.18;
        let cyp = a.head.cy - a.head.ry * 0.05;
        let ceil = rr.ceil() as i32;
        for i in 0..a.eyes {
            let off = (i as f32 - (a.eyes as f32 - 1.0) / 2.0) * spacing;
            let (mut ex, mut ey) = if a.eyes_vertical { (cxp, cyp + off) } else { (cxp + off, cyp) };
            ex = ex.clamp(a.head.cx - a.head.rx * 0.55, a.head.cx + a.head.rx * 0.72);
            ey = ey.clamp(a.head.cy - a.head.ry * 0.5, a.head.cy + a.head.ry * 0.5);
            if blink {
                for dx in -ceil..=ceil {
                    sp(ex as i32 + dx, ey as i32, a.outline);
                }
                continue;
            }
            for dy in -ceil..=ceil {
                for dx in -ceil..=ceil {
                    if (dx * dx + dy * dy) as f32 <= rr * rr + 0.5 {
                        sp(ex as i32 + dx, ey as i32 + dy, a.iris);
                    }
                }
            }
            sp(ex as i32, ey as i32, [0.05, 0.04, 0.08]);
            if rr >= 1.5 {
                sp(ex as i32 + 1, ey as i32, [0.05, 0.04, 0.08]);
            }
            sp(ex as i32, ey as i32 - 1, [1.0, 1.0, 1.0]);
        }
    }
    // anglerfish lure: a stalk arcing forward over the mouth with a glowing bulb
    if a.lure {
        let bx = a.head.cx + a.head.rx * 0.1;
        let by = a.head.cy - a.head.ry * 0.95;
        let tipx = a.head.cx + a.head.rx * (0.9 + a.lure_len);
        let tipy = a.head.cy - a.head.ry * 0.1 + s2 * 1.3; // bulb bobs
        let (cx2, cy2) = (bx + 2.0, by - a.lure_arc);
        for k in 0..=9 {
            let t = k as f32 / 9.0;
            let it = 1.0 - t;
            let x = it * it * bx + 2.0 * it * t * cx2 + t * t * tipx;
            let y = it * it * by + 2.0 * it * t * cy2 + t * t * tipy;
            sp(x as i32, y as i32, a.outline);
        }
        let glow = [1.0, 0.95, 0.5];
        for dy in -1..=1 {
            for dx in -1..=1 {
                sp(tipx as i32 + dx, tipy as i32 + dy, glow);
            }
        }
        sp(tipx as i32 - 1, tipy as i32 - 1, [1.0, 1.0, 0.92]);
    }
    // bioluminescent spots — brighten/dim with the idle loop
    if a.biolum_n > 0 {
        let pulse = (0.6 + 0.4 * s).clamp(0.0, 1.0); // s in [-1,1]
        let core = lighter(a.biolum_col, 0.25 * pulse);
        let halo = mix(darker(a.biolum_col, 0.45), a.biolum_col, pulse);
        for i in 0..a.biolum_n as usize {
            let (xi, yi) = (a.biolum_spots[i].0 as i32, a.biolum_spots[i].1 as i32);
            for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                sp(xi + dx, yi + dy, halo);
            }
            sp(xi, yi, core);
        }
    }
    img
}

fn upscale(img: &RgbaImage, s: u32) -> RgbaImage {
    imageops::resize(img, img.width() * s, img.height() * s, imageops::FilterType::Nearest)
}
fn cell_bg(cell: u32, i: u32) -> RgbaImage {
    // dark varied space tiles
    let mut bg = RgbaImage::new(cell, cell);
    let base = [[18u8, 16, 30], [26, 18, 28], [16, 22, 30], [24, 20, 22]][(i % 4) as usize];
    for p in bg.pixels_mut() {
        *p = Rgba([base[0], base[1], base[2], 255]);
    }
    bg
}

/// Build one individual of a genus. Same (genus_seed, indiv) => same creature.
fn creature(genus_seed: u64, indiv: u64) -> Alien {
    let mut grng = StdRng::seed_from_u64(genus_seed);
    let g = sample_genus(&mut grng);
    let mut rng = StdRng::seed_from_u64(genus_seed.wrapping_mul(1009) + indiv + 1);
    sample(&g, &mut rng)
}

/// Same, but force a specific PURE plan (both endpoints = plan) for the showcase.
fn creature_plan(genus_seed: u64, indiv: u64, plan: Plan) -> Alien {
    let mut grng = StdRng::seed_from_u64(genus_seed);
    let mut g = sample_genus(&mut grng);
    g.plan_a = plan;
    g.plan_b = plan;
    g.blend = 0.5;
    let mut rng = StdRng::seed_from_u64(genus_seed.wrapping_mul(1009) + indiv + 1);
    sample(&g, &mut rng)
}

/// One animation frame of a creature tile (bg + bobbing sprite).
fn frame_tile(a: &Alien, i: u32, up: u32, f: u32) -> RgbaImage {
    let cell = GRID * up;
    let phase = f as f32 / ANIM_FRAMES as f32;
    let tempo = a.anim_tempo as f32; // twitchy families run at double time
    let s = (TAU * phase * tempo).sin();
    let s2 = (TAU * phase * tempo).cos(); // out-of-phase signal for secondary motions
    let sprite = upscale(&draw(a, s, s2, a.blink_frame == f as i32), up);
    let oy = (a.bob_amp * s * up as f32).round() as i64;
    let mut tile = cell_bg(cell, i);
    imageops::overlay(&mut tile, &sprite, 0, oy);
    tile
}

fn write_gif(path: &str, frames: Vec<RgbaImage>, delay_ms: u32) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::create(path)?;
    let mut enc = GifEncoder::new(file);
    enc.set_repeat(Repeat::Infinite)?;
    for fr in frames {
        enc.encode_frame(Frame::from_parts(fr, 0, 0, Delay::from_numer_denom_ms(delay_ms, 1)))?;
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("out")?;
    let gutter = 2u32;

    // 1) genus sheet: each ROW is one genus (a planet's fauna), N individuals.
    {
        let cell = GRID * SHEET_UP;
        let (cols, rows) = (20u32, 20u32);
        let mut sheet = RgbaImage::new(cols * (cell + gutter) + gutter, rows * (cell + gutter) + gutter);
        for p in sheet.pixels_mut() {
            *p = Rgba([10, 9, 16, 255]);
        }
        for gi in 0..rows {
            for ci in 0..cols {
                let a = creature(9000 + gi as u64, ci as u64);
                let tile = frame_tile(&a, gi, SHEET_UP, 0); // bg keyed to genus row
                let x = gutter + ci * (cell + gutter);
                let y = gutter + gi * (cell + gutter);
                imageops::overlay(&mut sheet, &tile, x as i64, y as i64);
            }
        }
        sheet.save("out/aliens_genus.png")?;
        println!("Wrote out/aliens_genus.png ({rows} genera x {cols} individuals = {} creatures; each row = one family)", rows * cols);
    }

    // 1b) plan showcase: each ROW is one body plan (ostrich, penguin, dodo, ...).
    {
        let cell = GRID * SHEET_UP;
        let cols = 8u32;
        let rows = PLANS.len() as u32;
        let mut sheet = RgbaImage::new(cols * (cell + gutter) + gutter, rows * (cell + gutter) + gutter);
        for p in sheet.pixels_mut() {
            *p = Rgba([10, 9, 16, 255]);
        }
        for (pi, plan) in PLANS.iter().enumerate() {
            for ci in 0..cols {
                let a = creature_plan(12000 + pi as u64, ci as u64, *plan);
                let tile = frame_tile(&a, pi as u32, SHEET_UP, 0);
                let x = gutter + ci * (cell + gutter);
                let y = gutter + pi as u32 * (cell + gutter);
                imageops::overlay(&mut sheet, &tile, x as i64, y as i64);
            }
        }
        sheet.save("out/aliens_plans.png")?;
        println!("Wrote out/aliens_plans.png ({} pure plan endpoints, one per row)", PLANS.len());
    }

    // 2) animated grid GIF: 8 genera (rows) x 5 individuals, all idling.
    {
        let up = 3u32;
        let cell = GRID * up;
        let (cols, rows) = (5u32, 8u32);
        let creatures: Vec<Alien> = (0..rows)
            .flat_map(|gi| (0..cols).map(move |ci| creature(9000 + gi as u64, ci as u64)))
            .collect();
        let mut frames = Vec::new();
        for f in 0..ANIM_FRAMES {
            let mut sheet = RgbaImage::new(cols * (cell + gutter) + gutter, rows * (cell + gutter) + gutter);
            for p in sheet.pixels_mut() {
                *p = Rgba([10, 9, 16, 255]);
            }
            for (i, a) in creatures.iter().enumerate() {
                let tile = frame_tile(a, i as u32 / cols, up, f); // bg by genus row
                let x = gutter + (i as u32 % cols) * (cell + gutter);
                let y = gutter + (i as u32 / cols) * (cell + gutter);
                imageops::overlay(&mut sheet, &tile, x as i64, y as i64);
            }
            frames.push(sheet);
        }
        write_gif("out/aliens_anim.gif", frames, 110)?;
        println!("Wrote out/aliens_anim.gif ({} frames, grouped by genus)", ANIM_FRAMES);
    }

    // 3) a few big single-creature GIFs (varied genera)
    {
        let up = 6u32;
        for (k, (gs, ci)) in [(9001u64, 0u64), (9003, 1), (9004, 2), (9006, 0), (9002, 3), (9007, 1)].into_iter().enumerate() {
            let a = creature(gs, ci);
            let frames: Vec<RgbaImage> = (0..ANIM_FRAMES).map(|f| frame_tile(&a, k as u32, up, f)).collect();
            write_gif(&format!("out/alien_g{gs}_{ci}.gif"), frames, 110)?;
        }
        println!("Wrote out/alien_g<genus>_<i>.gif singles");
    }
    Ok(())
}
