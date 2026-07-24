//! Visual R&D — render several **beauty-render variants** of the galaxy map so a
//! direction can be picked by eye before any of it touches the shipped renderer.
//!
//! It reuses the real generation (`Galaxy::generate*` for node/edge/region data,
//! plus the pub `arms`/`twist`) and layers a *fresh, self-contained* beauty
//! renderer on top with per-variant feature toggles. Output: one contact sheet
//! per variant (a handful of seeds) + a same-seed comparison strip, into `out/`.
//!
//!   cargo run --release -p galaxy --bin variants
//!
//! Variants (see `VARIANTS`):
//!   A current      — today's shipped look (blocky haze + blooms + node dots)
//!   B star-field   — the galaxy body is thousands of unresolved stars on the
//!                    density field; nodes are a bright overlay
//!   C spiral+dust  — forced 2-arm spiral, bold arm contrast, dark dust lanes,
//!                    smooth (un-blocked) haze
//!   D full         — B + C + a bloom/glow post-pass + bright core
//!   E full+tilt    — D viewed as a tilted disc (3/4 view) for depth

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

/// Spiral-arm + core-bulge density (matches lib so the beauty field agrees with
/// where systems were placed).
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
/// A crisp **grand-design** spiral density: a low inter-arm floor and a high
/// ridge sharpness make the arms read as bold sweeping bands (vs. `density`'s
/// softer, cluster-like field). The arms fade in past the bar and out at the rim.
fn spiral_density(x: f32, y: f32, arms: u32, twist: f32, floor: f32, sharp: f32) -> f32 {
    let r = (x * x + y * y).sqrt();
    let rr = r / GR;
    let theta = y.atan2(x);
    let phase = theta * arms as f32 - twist * r.max(1.0).ln();
    let ridge = (0.5 + 0.5 * phase.cos()).powf(sharp); // crisp arm bands
    // Soften the ridge a touch with noise so arms aren't perfect sine bands.
    let n = fbm(x / 150.0, y / 150.0, 3);
    let arm = clamp01(ridge * (0.7 + 0.6 * n));
    // Arms live in an annulus: fade in past the bar, out toward the rim.
    let env = smoothstep(0.10, 0.26, rr) * smoothstep(1.02, 0.55, rr);
    clamp01((floor + (1.0 - floor) * arm) * env)
}
/// Dark dust-lane multiplier in ~[0.35, 1.0]: a second spiral offset in phase,
/// darkest on the leading edge of each arm at mid radius.
fn dust(x: f32, y: f32, arms: u32, twist: f32) -> f32 {
    let r = (x * x + y * y).sqrt();
    let rr = r / GR;
    let theta = y.atan2(x);
    let phase = theta * arms as f32 - twist * r.max(1.0).ln() + 0.7; // leading offset
    let ridge = 0.5 + 0.5 * phase.cos();
    let win = smoothstep(0.12, 0.30, rr) * smoothstep(0.95, 0.6, rr);
    let n = fbm(x / 90.0 + 20.0, y / 90.0, 3);
    let lane = smoothstep(0.55, 0.92, ridge) * win * (0.6 + 0.8 * n);
    1.0 - 0.62 * clamp01(lane)
}
/// Field-star colour by galactocentric radius (old warm core → young blue arms),
/// with an occasional pink H-II region.
fn star_color(x: f32, y: f32, rng: &mut R) -> Rgb {
    let rr = (x * x + y * y).sqrt() / GR;
    let base = mix([1.0, 0.86, 0.60], [0.74, 0.83, 1.0], smoothstep(0.12, 0.55, rr));
    if rng.f() < 0.05 {
        return [1.0, 0.55, 0.72]; // H-II pink
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
    /// Soft additive disc.
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
    /// Hard additive square (the chunky "current" haze block).
    fn block(&mut self, cx: f32, cy: f32, half: i32, c: Rgb, a: f32) {
        for dy in -half..=half {
            for dx in -half..=half {
                self.add(cx as i32 + dx, cy as i32 + dy, c, a);
            }
        }
    }
    /// Bloom: bright-pass → blur → add back, so bright cores/stars bleed light.
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
        // separable box blur, a few wide passes
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
    /// Tone-map (per-channel Reinhard roll-off) + gamma + vignette → RGBA8.
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
                    let m = e / (1.0 + e); // Reinhard
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
struct Style {
    tag: &'static str,
    force_arms: u32, // 0 = seed's own
    blocky_haze: bool,
    smooth_haze: bool,
    dust_lanes: bool,
    star_field: usize, // # unresolved field stars (0 = off)
    blooms: f32,       // nebula-pocket brightness (0 = off)
    bloom_post: f32,   // glow post-pass strength (0 = off)
    tilt: bool,
    core: f32,    // core-bulge brightness
    core_r: f32,  // core-bulge radius (fraction of GR)
    arm_pow: f32, // haze arm contrast exponent
    exposure: f32,
    // Bold-spiral controls (when `spiral`): swap the density field for a crisp
    // log-spiral with a low inter-arm floor + high ridge sharpness, and a bar.
    spiral: bool,
    arm_floor: f32,
    arm_sharp: f32,
    bar: bool,
}
const D0: Style = Style {
    tag: "", force_arms: 0, blocky_haze: false, smooth_haze: true, dust_lanes: false, star_field: 0,
    blooms: 0.0, bloom_post: 0.0, tilt: false, core: 1.5, core_r: 0.30, arm_pow: 1.7, exposure: 1.1,
    spiral: false, arm_floor: 0.05, arm_sharp: 5.0, bar: false,
};
const VARIANTS: &[Style] = &[
    Style { tag: "A_current", blocky_haze: true, smooth_haze: false, blooms: 0.5, core: 1.0, arm_pow: 1.4, exposure: 1.15, ..D0 },
    Style { tag: "B_starfield", star_field: 13000, blooms: 0.28, core: 1.5, ..D0 },
    Style { tag: "C_spiral_dust", force_arms: 2, dust_lanes: true, blooms: 0.30, core: 1.7, arm_pow: 2.3, ..D0 },
    Style { tag: "D_full", force_arms: 2, dust_lanes: true, star_field: 15000, blooms: 0.42, bloom_post: 0.7, core: 1.9, arm_pow: 2.3, exposure: 1.05, ..D0 },
    Style { tag: "E_full_tilt", force_arms: 2, dust_lanes: true, star_field: 15000, blooms: 0.42, bloom_post: 0.7, tilt: true, core: 1.9, arm_pow: 2.3, exposure: 1.05, ..D0 },
    // Genuine spiral: crisp arms (low floor, sharp ridge) + a central bar.
    Style { tag: "F_grand_spiral", force_arms: 2, dust_lanes: true, star_field: 17000, blooms: 0.40, bloom_post: 0.7, core: 1.7, core_r: 0.20, exposure: 1.05, spiral: true, arm_floor: 0.04, arm_sharp: 5.5, bar: true, ..D0 },
    Style { tag: "G_grand_spiral_tilt", force_arms: 2, dust_lanes: true, star_field: 17000, blooms: 0.40, bloom_post: 0.7, tilt: true, core: 1.7, core_r: 0.20, exposure: 1.05, spiral: true, arm_floor: 0.04, arm_sharp: 5.5, bar: true, ..D0 },
];

/// World → screen, optionally as a tilted 3/4 disc.
fn proj(wx: f32, wy: f32, cam: &Camera, w: u32, h: u32, tilt: bool) -> (f32, f32, f32) {
    let (dx, dy) = (wx - cam.x, wy - cam.y);
    if !tilt {
        (w as f32 * 0.5 + dx * cam.zoom, h as f32 * 0.5 + dy * cam.zoom, 1.0)
    } else {
        let depth = dy / GR; // front (+y) nearer
        let sc = 1.0 + 0.20 * depth;
        (w as f32 * 0.5 + dx * cam.zoom * sc, h as f32 * 0.5 + dy * cam.zoom * 0.52, sc)
    }
}

/// Nebula pockets on the arm ridges (own copy of lib's placement idea).
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
    let ext = gal.extent();
    let zoom = (0.46 * w as f32 / ext).min(0.46 * h as f32 / ext) * if st.tilt { 1.15 } else { 1.0 };
    let cam = Camera { x: 0.0, y: 0.0, zoom };
    let mut fb = Fb::new(w, h, [0.008, 0.010, 0.028]);

    // Density the beauty layer samples: the crisp grand-design spiral, or the
    // softer cluster field, times the dust lanes if enabled.
    let dens_fn = |wx: f32, wy: f32| -> f32 {
        let base = if st.spiral {
            spiral_density(wx, wy, arms, twist, st.arm_floor, st.arm_sharp)
        } else {
            density(wx, wy, arms, twist)
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
        let cells = (2.2 * ext / stepw) as i32;
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
                let k = d.powf(st.arm_pow) * 0.5;
                let (sx, sy, sc) = proj(wx, wy, &cam, w, h, st.tilt);
                if st.blocky_haze {
                    fb.block(sx, sy, (step_px * 0.6) as i32, col, k * 0.5);
                } else {
                    fb.splat(sx, sy, step_px * sc * 1.25, col, k, 1.4);
                }
            }
        }
    }

    // --- core bulge: the brightest thing on screen ---
    {
        let (sx, sy, _) = proj(0.0, 0.0, &cam, w, h, st.tilt);
        let rcore = st.core_r * GR * zoom;
        fb.splat(sx, sy, rcore, [1.0, 0.82, 0.46], 0.9 * st.core, 2.2);
        fb.splat(sx, sy, rcore * 0.45, [1.0, 0.92, 0.72], 1.1 * st.core, 1.6);
    }

    // --- central bar (barred spiral): an elongated bright bulge through the
    // core, its arms emerging from the ends. Drawn as overlapping splats along
    // a line so it tilts/blooms with everything else.
    if st.bar {
        let blen = 0.34 * GR;
        let steps = 26;
        for i in 0..=steps {
            let f = i as f32 / steps as f32 * 2.0 - 1.0; // −1..1 along the bar
            let (wx, wy) = (f * blen, 0.0);
            let (sx, sy, sc) = proj(wx, wy, &cam, w, h, st.tilt);
            let taper = (1.0 - f.abs() * 0.85).max(0.0);
            fb.splat(sx, sy, 0.09 * GR * zoom * sc * (0.6 + taper), [1.0, 0.84, 0.50], 0.30 * st.core * taper, 1.8);
        }
    }

    // --- nebula pockets ---
    if st.blooms > 0.0 {
        for (bx, by, br, col) in blooms(seed, arms, twist) {
            let (sx, sy, sc) = proj(bx, by, &cam, w, h, st.tilt);
            fb.splat(sx, sy, br * zoom * sc, col, st.blooms, 1.8);
        }
    }

    // --- dense unresolved field stars tracing the density ---
    if st.star_field > 0 {
        let mut rng = R(seed ^ 0x1234_abcd);
        let (mut placed, mut tries) = (0usize, 0usize);
        while placed < st.star_field && tries < st.star_field * 40 {
            tries += 1;
            let rad = GR * rng.f().sqrt();
            let ang = rng.f() * TAU;
            let (wx, wy) = (rad * ang.cos(), rad * ang.sin());
            let d = dens_fn(wx, wy);
            // Spiral variants concentrate stars onto the arms (higher gamma =
            // fewer inter-arm stars = crisper arms).
            let gamma = if st.spiral { 1.35 } else { 0.85 };
            if rng.f() > d.powf(gamma) {
                continue;
            }
            placed += 1;
            let col = star_color(wx, wy, &mut rng);
            let (sx, sy, sc) = proj(wx, wy, &cam, w, h, st.tilt);
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
        let (ax, ay, _) = proj(na.x, na.y, &cam, w, h, st.tilt);
        let (bx, by, _) = proj(nb.x, nb.y, &cam, w, h, st.tilt);
        let steps = ((bx - ax).abs().max((by - ay).abs())).ceil() as i32;
        if steps <= 0 {
            continue;
        }
        for s in 0..=steps {
            let t = s as f32 / steps as f32;
            fb.add(lerp(ax, bx, t) as i32, lerp(ay, by, t) as i32, [0.34, 0.40, 0.62], 0.05);
        }
    }

    // --- interactive systems as a bright overlay ---
    const SUN: &[Rgb] = &[
        [1.00, 0.90, 0.55], [1.00, 0.72, 0.42], [1.00, 0.54, 0.42], [0.93, 0.96, 1.00], [0.64, 0.80, 1.00],
    ];
    for nd in &gal.nodes {
        let (sx, sy, sc) = proj(nd.x, nd.y, &cam, w, h, st.tilt);
        let tint = SUN[nd.star as usize % SUN.len()];
        let core = (1.0 + 2.0 * nd.importance) * sc;
        fb.splat(sx, sy, core * 2.6, tint, 0.16, 1.6);
        fb.splat(sx, sy, core, tint, 0.9, 1.2);
        fb.splat(sx, sy, core * 0.5, [1.0, 1.0, 1.0], 0.9, 1.0);
        if nd.importance > 0.6 {
            let g = (core * 2.4).min(20.0);
            let n = g as i32;
            for k in 1..=n {
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

    // One contact sheet per variant (a handful of seeds each).
    println!("Variant contact sheets:");
    for st in VARIANTS {
        let cells: Vec<RgbaImage> = seeds.iter().map(|&s| render(s, st, cw, ch)).collect();
        let sheet = grid(&cells, 2, 10);
        let path = format!("out/var_{}.png", st.tag);
        sheet.save(&path)?;
        println!("  wrote {path}");
    }

    // Same-seed comparison strips: each row is one seed across all variants.
    println!("Comparison strips:");
    for &s in &[7u32, 42] {
        let cells: Vec<RgbaImage> = VARIANTS.iter().map(|st| render(s, st, cw, ch)).collect();
        let strip = grid(&cells, VARIANTS.len() as u32, 8);
        let path = format!("out/var_compare_seed{s}.png");
        strip.save(&path)?;
        println!("  wrote {path}  (L→R: {})", VARIANTS.iter().map(|v| v.tag).collect::<Vec<_>>().join(", "));
    }
    Ok(())
}
