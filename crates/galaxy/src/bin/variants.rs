//! Visual R&D — render several **beauty-render variants** of the galaxy map so a
//! direction can be picked by eye before any of it touches the shipped renderer.
//!
//! It reuses the real generation (`Galaxy::generate*` for node/edge/region data
//! plus the pub `arms`/`twist`) and layers a *fresh, self-contained* beauty
//! renderer on top with per-variant feature toggles. Output: one contact sheet
//! per variant (a handful of seeds) + a same-seed comparison strip, into `out/`.
//!
//!   cargo run --release -p galaxy --bin variants
//!
//! Current focus: an **M101 / Pinwheel**-style galaxy — multi-armed, flocculent
//! (patchy, feathered arms), lopsided, a small modest nucleus (no bar), studded
//! with bright pink **H II** star-forming knots — compared against the clean
//! grand-design barred spiral.

use galaxy::{Camera, Galaxy};
use image::RgbaImage;
use std::f32::consts::TAU;

// ---------- primitives (own copy; native-only) ----------
fn hash3(x: i32, y: i32, z: i32) -> f32 {
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
fn fbm(mut x: f32, mut y: f32, o: u32) -> f32 {
    let (mut s, mut a, mut n) = (0.0, 0.5, 0.0);
    for _ in 0..o {
        s += a * value_noise(x, y, 3.1);
        n += a;
        a *= 0.5;
        x *= 2.0;
        y *= 2.0;
    }
    s / n
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
/// A tiny local RNG for the beauty layer (seeded per variant/seed).
struct R(u32);
impl R {
    fn f(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 8) as f32 / ((1u32 << 24) as f32)
    }
    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.f()
    }
}

const GR: f32 = 1000.0; // galaxy radius (matches lib::GALAXY_R)

/// Cluster-ish spiral-arm + core-bulge density (matches lib).
fn density(x: f32, y: f32, arms: u32, twist: f32) -> f32 {
    let r = (x * x + y * y).sqrt();
    let rr = r / GR;
    let radial = smoothstep(1.05, 0.02, rr);
    let theta = y.atan2(x);
    let phase = theta * arms as f32 - twist * r.max(1.0).ln();
    let ridge = 0.5 + 0.5 * phase.cos();
    let arm = 0.22 + 0.78 * ridge.powf(2.2);
    let bulge = smoothstep(0.30, 0.0, rr);
    let n = fbm(x / 190.0, y / 190.0, 3);
    clamp01(radial * arm * (0.5 + 0.95 * n) + bulge * 0.85)
}

/// A crisp **grand-design** spiral density (low inter-arm floor, sharp ridge).
fn spiral_density(x: f32, y: f32, arms: u32, twist: f32, floor: f32, sharp: f32) -> f32 {
    let r = (x * x + y * y).sqrt();
    let rr = r / GR;
    let theta = y.atan2(x);
    let phase = theta * arms as f32 - twist * r.max(1.0).ln();
    let ridge = (0.5 + 0.5 * phase.cos()).powf(sharp);
    let n = fbm(x / 150.0, y / 150.0, 3);
    let arm = clamp01(ridge * (0.7 + 0.6 * n));
    let env = smoothstep(0.10, 0.26, rr) * smoothstep(1.02, 0.55, rr);
    clamp01((floor + (1.0 - floor) * arm) * env)
}

/// **M101 / Pinwheel** density: multi-armed but **flocculent** — the spiral phase
/// is domain-warped so arms wander and feather, then fragmented by higher-freq
/// noise into patches and spurs, and the whole disc is made **lopsided** (an m=1
/// asymmetry) with a small nucleus. `warp` bends the arms; `asym`/`asym_phi` push
/// mass to one side.
fn m101_density(x: f32, y: f32, arms: u32, twist: f32, floor: f32, warp: f32, asym: f32, asym_phi: f32) -> f32 {
    let r = (x * x + y * y).sqrt();
    let rr = r / GR;
    let theta = y.atan2(x);
    // Low-freq domain warp on the spiral phase → arms wander (not clean bands).
    let wf = (fbm(x / 240.0 + 5.0, y / 240.0, 2) - 0.5) * warp;
    let phase = theta * arms as f32 - twist * r.max(1.0).ln() + wf;
    let ridge = (0.5 + 0.5 * phase.cos()).powf(2.1);
    // Fragment the arms into patches/knots (the flocculent texture).
    let patch = fbm(x / 60.0 + 30.0, y / 60.0, 3);
    let arm = clamp01(ridge * (0.28 + 1.45 * patch));
    // Lopsided m=1 asymmetry: more disc on one side (M101's signature).
    let asy = (1.0 + asym * (theta - asym_phi).cos()).max(0.0);
    let env = smoothstep(0.05, 0.18, rr) * smoothstep(1.08, 0.52, rr);
    clamp01((floor + (1.0 - floor) * arm) * env * asy)
}

/// Dark dust-lane multiplier in ~[0.35, 1.0].
fn dust(x: f32, y: f32, arms: u32, twist: f32) -> f32 {
    let r = (x * x + y * y).sqrt();
    let rr = r / GR;
    let theta = y.atan2(x);
    let phase = theta * arms as f32 - twist * r.max(1.0).ln() + 0.7;
    let ridge = 0.5 + 0.5 * phase.cos();
    let win = smoothstep(0.12, 0.30, rr) * smoothstep(0.95, 0.6, rr);
    let n = fbm(x / 90.0 + 20.0, y / 90.0, 3);
    let lane = smoothstep(0.55, 0.92, ridge) * win * (0.6 + 0.8 * n);
    1.0 - 0.55 * clamp01(lane)
}

/// Field-star colour by galactocentric radius. `blue` biases the arms bluer
/// (young stars — M101's arms are notably blue).
fn star_color(x: f32, y: f32, blue: bool, rng: &mut R) -> Rgb {
    let rr = (x * x + y * y).sqrt() / GR;
    let (warm, edge) = if blue { ([1.0, 0.86, 0.62], 0.42) } else { ([1.0, 0.86, 0.60], 0.55) };
    let base = mix(warm, [0.70, 0.82, 1.0], smoothstep(0.10, edge, rr));
    if rng.f() < if blue { 0.07 } else { 0.05 } {
        return [1.0, 0.52, 0.66]; // H-II pink
    }
    base
}

// ---------- float accumulation buffer ----------
struct Fb {
    w: u32,
    h: u32,
    px: Vec<Rgb>,
}
impl Fb {
    fn new(w: u32, h: u32, base: Rgb) -> Fb {
        Fb { w, h, px: vec![base; (w * h) as usize] }
    }
    #[inline]
    fn add(&mut self, x: i32, y: i32, c: Rgb, a: f32) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 || a <= 0.0 {
            return;
        }
        let i = (y as u32 * self.w + x as u32) as usize;
        self.px[i][0] += c[0] * a;
        self.px[i][1] += c[1] * a;
        self.px[i][2] += c[2] * a;
    }
    fn splat(&mut self, cx: f32, cy: f32, rad: f32, c: Rgb, bright: f32, pow: f32) {
        if rad <= 0.0 || bright <= 0.0 {
            return;
        }
        let r = rad.ceil() as i32;
        let inv = 1.0 / rad;
        for dy in -r..=r {
            for dx in -r..=r {
                let d = ((dx * dx + dy * dy) as f32).sqrt() * inv;
                if d >= 1.0 {
                    continue;
                }
                let f = (1.0 - d).powf(pow);
                self.add(cx as i32 + dx, cy as i32 + dy, c, bright * f);
            }
        }
    }
    fn block(&mut self, cx: f32, cy: f32, half: i32, c: Rgb, a: f32) {
        for dy in -half..=half {
            for dx in -half..=half {
                self.add(cx as i32 + dx, cy as i32 + dy, c, a);
            }
        }
    }
    fn bloom(&mut self, thresh: f32, strength: f32) {
        let (w, h) = (self.w as usize, self.h as usize);
        let mut b: Vec<Rgb> = vec![[0.0; 3]; w * h];
        for i in 0..w * h {
            let l = self.px[i][0].max(self.px[i][1]).max(self.px[i][2]);
            if l > thresh {
                let k = l - thresh;
                b[i] = [self.px[i][0] * k, self.px[i][1] * k, self.px[i][2] * k];
            }
        }
        for _ in 0..3 {
            b = blur_h(&b, w, h, 6);
            b = blur_v(&b, w, h, 6);
        }
        for i in 0..w * h {
            self.px[i][0] += b[i][0] * strength;
            self.px[i][1] += b[i][1] * strength;
            self.px[i][2] += b[i][2] * strength;
        }
    }
    fn to_image(&self, exposure: f32) -> RgbaImage {
        let mut img = RgbaImage::new(self.w, self.h);
        let (cx, cy) = (self.w as f32 * 0.5, self.h as f32 * 0.5);
        let maxr = (cx * cx + cy * cy).sqrt();
        for y in 0..self.h {
            for x in 0..self.w {
                let c = self.px[(y * self.w + x) as usize];
                let vig = 1.0 - 0.32 * ((x as f32 - cx).powi(2) + (y as f32 - cy).powi(2)).sqrt() / maxr;
                let map = |v: f32| {
                    let e = (v * exposure).max(0.0);
                    let m = e / (1.0 + e);
                    (m.powf(1.0 / 2.2) * vig * 255.0).clamp(0.0, 255.0) as u8
                };
                img.put_pixel(x, y, image::Rgba([map(c[0]), map(c[1]), map(c[2]), 255]));
            }
        }
        img
    }
}
fn blur_h(src: &[Rgb], w: usize, h: usize, r: i32) -> Vec<Rgb> {
    let mut out = vec![[0.0; 3]; w * h];
    let norm = 1.0 / (2 * r + 1) as f32;
    for y in 0..h {
        for x in 0..w {
            let mut s = [0.0; 3];
            for dx in -r..=r {
                let xx = (x as i32 + dx).clamp(0, w as i32 - 1) as usize;
                let p = src[y * w + xx];
                s[0] += p[0];
                s[1] += p[1];
                s[2] += p[2];
            }
            out[y * w + x] = [s[0] * norm, s[1] * norm, s[2] * norm];
        }
    }
    out
}
fn blur_v(src: &[Rgb], w: usize, h: usize, r: i32) -> Vec<Rgb> {
    let mut out = vec![[0.0; 3]; w * h];
    let norm = 1.0 / (2 * r + 1) as f32;
    for y in 0..h {
        for x in 0..w {
            let mut s = [0.0; 3];
            for dy in -r..=r {
                let yy = (y as i32 + dy).clamp(0, h as i32 - 1) as usize;
                let p = src[yy * w + x];
                s[0] += p[0];
                s[1] += p[1];
                s[2] += p[2];
            }
            out[y * w + x] = [s[0] * norm, s[1] * norm, s[2] * norm];
        }
    }
    out
}

// ---------- variant description ----------
#[derive(Clone, Copy)]
enum Field {
    Cluster,
    Spiral,
    M101,
}
#[derive(Clone, Copy)]
struct Style {
    tag: &'static str,
    field: Field,
    force_arms: u32,
    blocky_haze: bool,
    smooth_haze: bool,
    dust_lanes: bool,
    star_field: usize,
    blooms: f32,     // arm-ridge nebula pockets (0 = off)
    hii: usize,      // # bright H II knots (M101 signature; 0 = off)
    bloom_post: f32, // glow post-pass strength
    squash: f32,     // 1.0 = face-on; < 1 tilts the disc
    persp: f32,      // extra near/far scale under tilt
    core: f32,
    core_r: f32,
    bar: bool,
    exposure: f32,
    // field params
    arm_floor: f32,
    arm_sharp: f32,
    warp: f32,
    asym: f32,
    blue_arms: bool,
}
const D0: Style = Style {
    tag: "", field: Field::Cluster, force_arms: 0, blocky_haze: false, smooth_haze: true,
    dust_lanes: false, star_field: 0, blooms: 0.0, hii: 0, bloom_post: 0.0, squash: 1.0, persp: 0.0,
    core: 1.5, core_r: 0.30, bar: false, exposure: 1.08, arm_floor: 0.05, arm_sharp: 5.0, warp: 0.0,
    asym: 0.0, blue_arms: false,
};
const VARIANTS: &[Style] = &[
    // The clean grand-design barred spiral, kept as the "explicit spiral" the
    // M101 look is being compared against.
    Style { tag: "F_grand_spiral", field: Field::Spiral, force_arms: 2, dust_lanes: true, star_field: 16000, blooms: 0.40, bloom_post: 0.7, core: 1.7, core_r: 0.20, bar: true, exposure: 1.05, arm_floor: 0.04, arm_sharp: 5.5, ..D0 },
    // M101: multi-armed, flocculent, lopsided, small core, H II knots.
    Style { tag: "M101_faceon", field: Field::M101, force_arms: 4, dust_lanes: true, star_field: 17000, blooms: 0.0, hii: 60, bloom_post: 0.6, core: 1.0, core_r: 0.12, exposure: 1.05, arm_floor: 0.03, warp: 2.6, asym: 0.35, blue_arms: true, ..D0 },
    // Gently inclined (~25°) — still easy to click/pan on.
    Style { tag: "M101_incline", field: Field::M101, force_arms: 4, dust_lanes: true, star_field: 17000, blooms: 0.0, hii: 60, bloom_post: 0.6, squash: 0.82, persp: 0.10, core: 1.0, core_r: 0.12, exposure: 1.05, arm_floor: 0.03, warp: 2.6, asym: 0.35, blue_arms: true, ..D0 },
    // Grander / more open — fewer arms (3), stronger lopsidedness & feathering.
    Style { tag: "M101_open", field: Field::M101, force_arms: 3, dust_lanes: true, star_field: 17000, blooms: 0.0, hii: 70, bloom_post: 0.6, core: 1.0, core_r: 0.13, exposure: 1.05, arm_floor: 0.03, warp: 3.2, asym: 0.5, blue_arms: true, ..D0 },
];

/// World → screen, optionally as a tilted disc (`squash` < 1, plus perspective).
fn proj(wx: f32, wy: f32, cam: &Camera, w: u32, h: u32, st: &Style) -> (f32, f32, f32) {
    let (dx, dy) = (wx - cam.x, wy - cam.y);
    let depth = dy / GR;
    let sc = 1.0 + st.persp * depth;
    (w as f32 * 0.5 + dx * cam.zoom * sc, h as f32 * 0.5 + dy * cam.zoom * st.squash, sc)
}

/// Nebula pockets on the arm ridges (for the non-M101 variants).
fn blooms(seed: u32, arms: u32, twist: f32) -> Vec<(f32, f32, f32, Rgb)> {
    const NEB: &[Rgb] = &[
        [0.92, 0.26, 0.66], [0.98, 0.44, 0.60], [0.26, 0.82, 0.92],
        [0.60, 0.34, 0.92], [0.96, 0.32, 0.44], [0.22, 0.80, 0.72],
    ];
    let mut rng = R(seed ^ 0x8b_ad_f0_0d);
    let n = 12 + (rng.f() * 8.0) as usize;
    let mut v = Vec::new();
    for _ in 0..n {
        let r = rng.range(0.18, 0.95) * GR;
        let k = (rng.f() * arms as f32) as u32;
        let theta = (TAU * k as f32 + twist * r.max(1.0).ln()) / arms as f32 + rng.range(-0.16, 0.16);
        let (s, c) = theta.sin_cos();
        v.push((c * r + rng.range(-35.0, 35.0), s * r + rng.range(-35.0, 35.0), rng.range(70.0, 180.0), NEB[(rng.f() * 6.0) as usize % 6]));
    }
    v
}

fn haze_tint(rr: f32) -> Rgb {
    let core = [1.00, 0.80, 0.40];
    let mid = [0.22, 0.46, 0.86];
    let rim = [0.28, 0.62, 0.74];
    if rr < 0.34 {
        mix(core, mid, rr / 0.34)
    } else {
        mix(mid, rim, (rr - 0.34) / 0.66)
    }
}

fn render(seed: u32, st: &Style, w: u32, h: u32) -> RgbaImage {
    let gal = if st.force_arms > 0 {
        Galaxy::generate_params(seed, 0, -1.0, st.force_arms)
    } else {
        Galaxy::generate(seed)
    };
    let (arms, twist) = (gal.arms, gal.twist);
    let asym_phi = hash3(seed as i32, 3, 9) * TAU; // lopsided direction, per galaxy
    let ext = gal.extent();
    // A squashed (tilted) disc is shorter, so zoom in a touch to fill the frame.
    let zoom = (0.46 * w as f32 / ext).min(0.46 * h as f32 / ext) / st.squash.clamp(0.6, 1.0);
    let cam = Camera { x: 0.0, y: 0.0, zoom };
    let mut fb = Fb::new(w, h, [0.008, 0.010, 0.028]);

    // Density the beauty layer samples (× dust lanes if enabled).
    let dens_fn = |wx: f32, wy: f32| -> f32 {
        let base = match st.field {
            Field::Cluster => density(wx, wy, arms, twist),
            Field::Spiral => spiral_density(wx, wy, arms, twist, st.arm_floor, st.arm_sharp),
            Field::M101 => m101_density(wx, wy, arms, twist, st.arm_floor, st.warp, st.asym, asym_phi),
        };
        if st.dust_lanes {
            base * dust(wx, wy, arms, twist)
        } else {
            base
        }
    };

    // --- haze (forward world-grid splats so tilt just works) ---
    if st.blocky_haze || st.smooth_haze {
        let step_px = if st.blocky_haze { 9.0 } else { 7.0 };
        let stepw = step_px / zoom;
        let cells = (2.4 * ext / stepw) as i32;
        let half = (cells / 2).max(1);
        for cy in -half..=half {
            for cx in -half..=half {
                let (wx, wy) = (cx as f32 * stepw, cy as f32 * stepw);
                let d = dens_fn(wx, wy);
                if d < 0.015 {
                    continue;
                }
                let rr = (wx * wx + wy * wy).sqrt() / GR;
                let col = haze_tint(rr.min(1.0));
                let k = d.powf(1.4) * 0.45;
                let (sx, sy, sc) = proj(wx, wy, &cam, w, h, st);
                if st.blocky_haze {
                    fb.block(sx, sy, (step_px * 0.6) as i32, col, k * 0.5);
                } else {
                    fb.splat(sx, sy, step_px * sc * 1.25, col, k, 1.4);
                }
            }
        }
    }

    // --- nucleus (small + modest for M101; bigger/brighter otherwise) ---
    {
        let (sx, sy, _) = proj(0.0, 0.0, &cam, w, h, st);
        let rcore = st.core_r * GR * zoom;
        let tint = if matches!(st.field, Field::M101) { [1.0, 0.88, 0.62] } else { [1.0, 0.82, 0.46] };
        fb.splat(sx, sy, rcore, tint, 0.85 * st.core, 2.2);
        fb.splat(sx, sy, rcore * 0.45, [1.0, 0.93, 0.78], 1.05 * st.core, 1.6);
    }

    // --- central bar (barred spiral only) ---
    if st.bar {
        let blen = 0.34 * GR;
        for i in 0..=26 {
            let f = i as f32 / 26.0 * 2.0 - 1.0;
            let (sx, sy, sc) = proj(f * blen, 0.0, &cam, w, h, st);
            let taper = (1.0 - f.abs() * 0.85).max(0.0);
            fb.splat(sx, sy, 0.09 * GR * zoom * sc * (0.6 + taper), [1.0, 0.84, 0.50], 0.30 * st.core * taper, 1.8);
        }
    }

    // --- arm-ridge nebula pockets (non-M101) ---
    if st.blooms > 0.0 {
        for (bx, by, br, col) in blooms(seed, arms, twist) {
            let (sx, sy, sc) = proj(bx, by, &cam, w, h, st);
            fb.splat(sx, sy, br * zoom * sc, col, st.blooms, 1.8);
        }
    }

    // --- H II knots: bright pink/red star-forming regions strung along the arms
    // (M101's signature). Rejection-sampled from the density so they hug arms;
    // a few are giant + very bright (like NGC 5471). ---
    if st.hii > 0 {
        const HII: &[Rgb] = &[
            [1.0, 0.42, 0.52], [1.0, 0.55, 0.42], [0.95, 0.35, 0.60],
            [1.0, 0.48, 0.48], [0.72, 0.86, 1.0], // an occasional blue OB knot
        ];
        let mut rng = R(seed ^ 0x5a_5a_11_01);
        let (mut placed, mut tries) = (0usize, 0usize);
        while placed < st.hii && tries < st.hii * 60 {
            tries += 1;
            let rad = GR * rng.range(0.12, 1.0);
            let ang = rng.f() * TAU;
            let (wx, wy) = (rad * ang.cos(), rad * ang.sin());
            if rng.f() > dens_fn(wx, wy).powf(0.7) {
                continue;
            }
            placed += 1;
            let (sx, sy, sc) = proj(wx, wy, &cam, w, h, st);
            let giant = rng.f() < 0.14;
            let rr = if giant { rng.range(60.0, 110.0) } else { rng.range(16.0, 42.0) };
            let col = HII[(rng.f() * HII.len() as f32) as usize % HII.len()];
            let b = if giant { 0.75 } else { 0.42 };
            fb.splat(sx, sy, rr * zoom * sc, col, b, 1.7);
            // a hot white-ish core to the knot
            fb.splat(sx, sy, rr * zoom * sc * 0.4, [1.0, 0.9, 0.9], b * 0.8, 1.3);
        }
    }

    // --- dense unresolved field stars tracing the density ---
    if st.star_field > 0 {
        let mut rng = R(seed ^ 0x1234_abcd);
        let (mut placed, mut tries) = (0usize, 0usize);
        let gamma = match st.field {
            Field::Spiral => 1.35,
            Field::M101 => 1.15,
            Field::Cluster => 0.85,
        };
        while placed < st.star_field && tries < st.star_field * 40 {
            tries += 1;
            let rad = GR * rng.f().sqrt();
            let ang = rng.f() * TAU;
            let (wx, wy) = (rad * ang.cos(), rad * ang.sin());
            let d = dens_fn(wx, wy);
            if rng.f() > d.powf(gamma) {
                continue;
            }
            placed += 1;
            let col = star_color(wx, wy, st.blue_arms, &mut rng);
            let (sx, sy, sc) = proj(wx, wy, &cam, w, h, st);
            let b = rng.range(0.10, 0.55);
            if rng.f() < 0.05 {
                fb.splat(sx, sy, 1.7 * sc, col, b * 1.1, 1.0);
            } else {
                fb.add(sx as i32, sy as i32, col, b * 0.6);
            }
        }
    }

    // --- faint hyperlanes (keep the "it's a map" read, but quiet) ---
    for &(a, b) in &gal.edges {
        let na = &gal.nodes[a as usize];
        let nb = &gal.nodes[b as usize];
        let (ax, ay, _) = proj(na.x, na.y, &cam, w, h, st);
        let (bx, by, _) = proj(nb.x, nb.y, &cam, w, h, st);
        let steps = ((bx - ax).abs().max((by - ay).abs())).ceil() as i32;
        if steps <= 0 {
            continue;
        }
        for s in 0..=steps {
            let t = s as f32 / steps as f32;
            fb.add(lerp(ax, bx, t) as i32, lerp(ay, by, t) as i32, [0.34, 0.40, 0.62], 0.045);
        }
    }

    // --- interactive systems as a bright overlay ---
    const SUN: &[Rgb] = &[
        [1.00, 0.90, 0.55], [1.00, 0.72, 0.42], [1.00, 0.54, 0.42], [0.93, 0.96, 1.00], [0.64, 0.80, 1.00],
    ];
    for nd in &gal.nodes {
        let (sx, sy, sc) = proj(nd.x, nd.y, &cam, w, h, st);
        let tint = SUN[nd.star as usize % SUN.len()];
        let core = (1.0 + 2.0 * nd.importance) * sc;
        fb.splat(sx, sy, core * 2.6, tint, 0.16, 1.6);
        fb.splat(sx, sy, core, tint, 0.9, 1.2);
        fb.splat(sx, sy, core * 0.5, [1.0, 1.0, 1.0], 0.9, 1.0);
        if nd.importance > 0.6 {
            let g = (core * 2.4).min(20.0);
            for k in 1..=(g as i32) {
                let f = (1.0 - k as f32 / g).max(0.0).powi(2) * 0.5;
                fb.add(sx as i32 + k, sy as i32, tint, f);
                fb.add(sx as i32 - k, sy as i32, tint, f);
                fb.add(sx as i32, sy as i32 + k, tint, f);
                fb.add(sx as i32, sy as i32 - k, tint, f);
            }
        }
    }

    if st.bloom_post > 0.0 {
        fb.bloom(0.85, st.bloom_post);
    }
    fb.to_image(st.exposure)
}

// ---------- image composition ----------
fn grid(cells: &[RgbaImage], cols: u32, gap: u32) -> RgbaImage {
    let (cw, ch) = (cells[0].width(), cells[0].height());
    let rows = (cells.len() as u32).div_ceil(cols);
    let gap_c: image::Rgba<u8> = image::Rgba([8, 8, 14, 255]);
    let mut out = RgbaImage::from_pixel(cols * cw + (cols + 1) * gap, rows * ch + (rows + 1) * gap, gap_c);
    for (i, c) in cells.iter().enumerate() {
        let (r, col) = (i as u32 / cols, i as u32 % cols);
        let (ox, oy) = (gap + col * (cw + gap), gap + r * (ch + gap));
        for y in 0..ch {
            for x in 0..cw {
                out.put_pixel(ox + x, oy + y, *c.get_pixel(x, y));
            }
        }
    }
    out
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("out")?;
    let seeds = [7u32, 42, 2024, 88];
    let (cw, ch) = (460u32, 460u32);

    println!("Variant contact sheets:");
    for st in VARIANTS {
        let cells: Vec<RgbaImage> = seeds.iter().map(|&s| render(s, st, cw, ch)).collect();
        grid(&cells, 2, 10).save(format!("out/var_{}.png", st.tag))?;
        println!("  wrote out/var_{}.png", st.tag);
    }

    println!("Comparison strips:");
    for &s in &[7u32, 42] {
        let cells: Vec<RgbaImage> = VARIANTS.iter().map(|st| render(s, st, cw, ch)).collect();
        grid(&cells, VARIANTS.len() as u32, 8).save(format!("out/var_compare_seed{s}.png"))?;
        println!("  wrote out/var_compare_seed{s}.png  (L→R: {})", VARIANTS.iter().map(|v| v.tag).collect::<Vec<_>>().join(", "));
    }
    Ok(())
}
