//! Procedural bird-sprite generator (side profile, ~32px, Neorice-style).
//!
//! A bird is drawn from shape PRIMITIVES whose proportions come from the seed,
//! biased by an ARCHETYPE (songbird, duck, raptor, owl, parrot, penguin ...).
//! Pipeline per bird:
//!   1. paint silhouette: tail -> neck -> body -> wing -> head -> beak
//!      (ellipses get soft rounded shading; flat parts get a 2-tone top light)
//!   2. 1px dark outline around the whole silhouette
//!   3. details on top: eye, crest/comb/ear-tufts, thin legs + feet
//!
//! Same seed => same bird. Adding a species = adding an `Arch` case. No assets.

use image::{imageops, Rgba, RgbaImage};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};

const W: u32 = 40;
const H: u32 = 40;
const SHEET_UP: u32 = 3;

type Rgb = [f32; 3];

// --------------------------------------------------------------------------
// small helpers
// --------------------------------------------------------------------------
fn mix(a: Rgb, b: Rgb, t: f32) -> Rgb {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
    ]
}
fn clamp01(x: f32) -> f32 {
    x.max(0.0).min(1.0)
}
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp01((x - e0) / (e1 - e0));
    t * t * (3.0 - 2.0 * t)
}
fn to_rgba(c: Rgb) -> Rgba<u8> {
    Rgba([
        (clamp01(c[0]) * 255.0) as u8,
        (clamp01(c[1]) * 255.0) as u8,
        (clamp01(c[2]) * 255.0) as u8,
        255,
    ])
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
    mix(c, [0.05, 0.05, 0.09], k)
}
fn lighter(c: Rgb, k: f32) -> Rgb {
    mix(c, [1.0, 1.0, 1.0], k)
}

/// distance from point p to segment a-b
fn seg_dist(px: f32, py: f32, ax: f32, ay: f32, bx: f32, by: f32) -> f32 {
    let (dx, dy) = (bx - ax, by - ay);
    let l2 = dx * dx + dy * dy;
    let t = if l2 <= 0.0 {
        0.0
    } else {
        (((px - ax) * dx + (py - ay) * dy) / l2).clamp(0.0, 1.0)
    };
    let (cx, cy) = (ax + t * dx, ay + t * dy);
    ((px - cx).powi(2) + (py - cy).powi(2)).sqrt()
}

// --------------------------------------------------------------------------
// canvas with an occupancy mask, so we can outline the silhouette
// --------------------------------------------------------------------------
struct Canvas {
    col: Vec<Rgb>,
    filled: Vec<bool>,
}
impl Canvas {
    fn new() -> Self {
        Canvas {
            col: vec![[0.0; 3]; (W * H) as usize],
            filled: vec![false; (W * H) as usize],
        }
    }
    fn idx(x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 || x >= W as i32 || y >= H as i32 {
            None
        } else {
            Some((y as u32 * W + x as u32) as usize)
        }
    }
    fn put(&mut self, x: i32, y: i32, c: Rgb) {
        if let Some(i) = Self::idx(x, y) {
            self.col[i] = c;
            self.filled[i] = true;
        }
    }
    fn is_filled(&self, x: i32, y: i32) -> bool {
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

/// Paint an ellipse blob with a top->bottom color blend + soft rounded shading.
fn blob(cv: &mut Canvas, e: Ell, c_top: Rgb, c_bot: Rgb) {
    // light from upper-right-front (screen y is down, so "up" = -y)
    let l = [0.30, -0.82, 0.49];
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
            let base = mix(c_top, c_bot, vt);
            let diff = nx * l[0] + ny * l[1] + nz * l[2];
            let lit = 0.55 + 0.5 * diff;
            let col = if lit > 0.86 {
                lighter(base, 0.22)
            } else if lit < 0.48 {
                darker(base, 0.26)
            } else {
                base
            };
            cv.put(x, y, col);
        }
    }
}

/// Capsule (thick segment) — necks and legs.
fn capsule(cv: &mut Canvas, ax: f32, ay: f32, bx: f32, by: f32, r: f32, c_top: Rgb, c_bot: Rgb) {
    let x0 = (ax.min(bx) - r - 1.0).floor() as i32;
    let x1 = (ax.max(bx) + r + 1.0).ceil() as i32;
    let y0 = (ay.min(by) - r - 1.0).floor() as i32;
    let y1 = (ay.max(by) + r + 1.0).ceil() as i32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            let d = seg_dist(x as f32 + 0.5, y as f32 + 0.5, ax, ay, bx, by);
            if d <= r {
                let side = clamp01(0.5 - (d / r.max(0.001)) * 0.5 + 0.25);
                cv.put(x, y, mix(c_bot, c_top, side));
            }
        }
    }
}

fn tri(cv: &mut Canvas, p: [(f32, f32); 3], c: Rgb, c_edge: Rgb) {
    let xs = [p[0].0, p[1].0, p[2].0];
    let ys = [p[0].1, p[1].1, p[2].1];
    let x0 = xs.iter().cloned().fold(f32::MAX, f32::min).floor() as i32 - 1;
    let x1 = xs.iter().cloned().fold(f32::MIN, f32::max).ceil() as i32 + 1;
    let y0 = ys.iter().cloned().fold(f32::MAX, f32::min).floor() as i32 - 1;
    let y1 = ys.iter().cloned().fold(f32::MIN, f32::max).ceil() as i32 + 1;
    let area = |ax: f32, ay: f32, bx: f32, by: f32, cx: f32, cy: f32| {
        (bx - ax) * (cy - ay) - (by - ay) * (cx - ax)
    };
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
            let d1 = area(p[0].0, p[0].1, p[1].0, p[1].1, fx, fy);
            let d2 = area(p[1].0, p[1].1, p[2].0, p[2].1, fx, fy);
            let d3 = area(p[2].0, p[2].1, p[0].0, p[0].1, fx, fy);
            let neg = (d1 < 0.0) || (d2 < 0.0) || (d3 < 0.0);
            let pos = (d1 > 0.0) || (d2 > 0.0) || (d3 > 0.0);
            if !(neg && pos) {
                // near an edge -> edge shade
                let m = d1.abs().min(d2.abs()).min(d3.abs());
                cv.put(x, y, if m < 1.2 { c_edge } else { c });
            }
        }
    }
}

// --------------------------------------------------------------------------
// bird definition
// --------------------------------------------------------------------------
#[derive(Clone, Copy, PartialEq)]
enum Crest {
    None,
    Tuft,
    Big,
    Comb,
    Ear,
}

struct Bird {
    body: Ell,
    head: Ell,
    neck: Option<(f32, f32, f32, f32, f32)>,
    beak_len: f32,
    beak_half: f32,
    beak_droop: f32,
    beak_hook: bool,
    beak_up: f32,
    tail_len: f32,
    tail_ang: f32,
    tail_half: f32,
    tail_fork: bool,
    wing: Ell,
    legs: bool,
    leg_x: [f32; 2],
    leg_len: f32,
    // colors
    back: Rgb,
    belly: Rgb,
    wing_col: Rgb,
    cap: Rgb,
    beak_col: Rgb,
    leg_col: Rgb,
    accent: Rgb,
    outline: Rgb,
    eye_white: bool,
    big_eye: bool,
    face_disk: Option<Rgb>,
    crest: Crest,
}

#[derive(Clone, Copy)]
enum Arch {
    Songbird,
    Duck,
    Goose,
    Raptor,
    Owl,
    Parrot,
    Penguin,
    Chicken,
    Chick,
    Wader,
}
const ARCHES: [Arch; 10] = [
    Arch::Songbird,
    Arch::Duck,
    Arch::Goose,
    Arch::Raptor,
    Arch::Owl,
    Arch::Parrot,
    Arch::Penguin,
    Arch::Chicken,
    Arch::Chick,
    Arch::Wader,
];

fn r(rng: &mut StdRng, a: f32, b: f32) -> f32 {
    a + (b - a) * rng.gen::<f32>()
}

fn sample(arch: Arch, rng: &mut StdRng) -> Bird {
    // sensible defaults (a small songbird), overridden per archetype
    let cx = 17.0;
    let cy = 23.0;
    let mut b = Bird {
        body: Ell { cx, cy, rx: 8.0, ry: 6.5 },
        head: Ell { cx: cx + 8.0, cy: cy - 8.0, rx: 4.6, ry: 4.6 },
        neck: None,
        beak_len: 4.0,
        beak_half: 1.6,
        beak_droop: 0.0,
        beak_hook: false,
        beak_up: 0.0,
        tail_len: 7.0,
        tail_ang: 0.15,
        tail_half: 2.2,
        tail_fork: false,
        wing: Ell { cx: cx - 1.0, cy: cy + 0.5, rx: 5.0, ry: 3.6 },
        legs: true,
        leg_x: [cx - 1.0, cx + 2.5],
        leg_len: 5.0,
        back: [0.4, 0.4, 0.45],
        belly: [0.9, 0.9, 0.9],
        wing_col: [0.3, 0.3, 0.35],
        cap: [0.3, 0.3, 0.35],
        beak_col: [0.95, 0.7, 0.2],
        leg_col: [0.9, 0.6, 0.2],
        accent: [0.9, 0.3, 0.2],
        outline: [0.12, 0.10, 0.14],
        eye_white: false,
        big_eye: false,
        face_disk: None,
        crest: Crest::None,
    };

    // palette pieces
    let hue = rng.gen::<f32>();
    let bright = |h: f32, rng: &mut StdRng| hsv(h, r(rng, 0.55, 0.8), r(rng, 0.6, 0.8));

    match arch {
        Arch::Songbird => {
            b.back = bright(hue, rng);
            b.belly = lighter(hsv((hue + 0.1).fract(), 0.2, 0.95), 0.1);
            b.wing_col = darker(b.back, 0.25);
            b.cap = if rng.gen_bool(0.5) { darker(b.back, 0.5) } else { b.back };
            b.beak_col = [0.2, 0.16, 0.14];
            b.leg_col = [0.5, 0.35, 0.3];
            b.tail_len = r(rng, 6.0, 10.0);
            b.tail_ang = r(rng, -0.1, 0.4);
            b.body.ry = r(rng, 6.0, 7.5);
            b.head.rx = r(rng, 4.2, 5.0);
            b.head.ry = b.head.rx;
            if rng.gen_bool(0.25) {
                b.crest = Crest::Tuft;
            }
        }
        Arch::Duck => {
            b.body = Ell { cx: 16.0, cy: 24.0, rx: 10.5, ry: 6.0 };
            b.head = Ell { cx: 26.5, cy: 16.0, rx: 4.4, ry: 4.4 };
            b.neck = Some((22.0, 20.0, 26.0, 17.0, 3.2));
            b.beak_len = 5.5;
            b.beak_half = 2.4;
            b.beak_droop = 0.6;
            b.tail_len = 5.0;
            b.tail_ang = 0.5;
            b.wing = Ell { cx: 14.0, cy: 23.0, rx: 6.5, ry: 4.0 };
            b.legs = true;
            b.leg_len = 3.0;
            b.leg_x = [15.0, 18.5];
            b.beak_col = [0.95, 0.72, 0.2];
            b.leg_col = [0.95, 0.6, 0.15];
            let mallard = rng.gen_bool(0.5);
            if mallard {
                b.back = hsv(0.09, 0.35, 0.5);
                b.belly = hsv(0.09, 0.2, 0.7);
                b.wing_col = hsv(0.09, 0.4, 0.4);
                b.cap = hsv(0.36, 0.6, 0.45); // green head
            } else {
                let g = r(rng, 0.85, 0.98);
                b.back = [g, g, g * 0.97];
                b.belly = [1.0, 1.0, 0.98];
                b.wing_col = [g * 0.9, g * 0.9, g * 0.88];
                b.cap = b.back;
            }
        }
        Arch::Goose => {
            b.body = Ell { cx: 15.0, cy: 25.0, rx: 9.5, ry: 6.5 };
            b.head = Ell { cx: 28.0, cy: 12.0, rx: 3.6, ry: 3.8 };
            b.neck = Some((20.0, 21.0, 28.0, 13.5, 2.6));
            b.beak_len = 4.0;
            b.beak_half = 1.8;
            b.tail_len = 5.0;
            b.tail_ang = 0.3;
            b.wing = Ell { cx: 13.0, cy: 24.0, rx: 6.0, ry: 4.2 };
            b.leg_len = 4.0;
            b.leg_x = [14.0, 17.5];
            let grey = rng.gen_bool(0.6);
            if grey {
                let g = r(rng, 0.5, 0.7);
                b.back = [g, g * 0.95, g * 0.88];
                b.belly = [0.92, 0.9, 0.85];
                b.wing_col = darker(b.back, 0.3);
                b.cap = darker(b.back, 0.4);
                b.beak_col = [0.15, 0.13, 0.13];
                b.leg_col = [0.2, 0.2, 0.22];
            } else {
                b.back = [0.95, 0.95, 0.93];
                b.belly = [1.0, 1.0, 0.99];
                b.wing_col = [0.86, 0.86, 0.84];
                b.cap = b.back;
                b.beak_col = [0.95, 0.6, 0.2];
                b.leg_col = [0.95, 0.6, 0.2];
            }
        }
        Arch::Raptor => {
            b.body = Ell { cx: 16.0, cy: 23.0, rx: 8.5, ry: 8.0 };
            b.head = Ell { cx: 24.0, cy: 13.0, rx: 5.0, ry: 4.8 };
            b.beak_len = 4.0;
            b.beak_half = 2.2;
            b.beak_hook = true;
            b.beak_droop = 0.4;
            b.tail_len = 8.0;
            b.tail_ang = -0.05;
            b.tail_half = 3.0;
            b.wing = Ell { cx: 14.0, cy: 22.0, rx: 6.5, ry: 5.5 };
            b.leg_col = [0.95, 0.7, 0.2];
            b.beak_col = [0.95, 0.72, 0.2];
            b.leg_len = 4.0;
            let eagle = rng.gen_bool(0.4);
            let br = r(rng, 0.28, 0.42);
            b.back = [br, br * 0.7, br * 0.45];
            b.wing_col = darker(b.back, 0.3);
            b.belly = if eagle {
                [br * 0.8, br * 0.6, br * 0.4]
            } else {
                [0.75, 0.62, 0.42]
            };
            b.cap = if eagle { [0.95, 0.95, 0.93] } else { darker(b.back, 0.2) };
            b.eye_white = true;
        }
        Arch::Owl => {
            // upright, big round head sitting low on a stout body
            b.body = Ell { cx: 19.0, cy: 26.0, rx: 8.0, ry: 8.0 };
            b.head = Ell { cx: 20.0, cy: 14.0, rx: 7.5, ry: 6.5 };
            b.beak_len = 2.0;
            b.beak_half = 1.4;
            b.beak_hook = true;
            b.beak_up = 1.0;
            b.tail_len = 3.0;
            b.tail_ang = -0.7;
            b.wing = Ell { cx: 15.5, cy: 26.0, rx: 5.5, ry: 6.5 };
            b.legs = true;
            b.leg_len = 2.0;
            b.leg_x = [17.0, 21.0];
            b.crest = Crest::Ear;
            let g = r(rng, 0.35, 0.52);
            b.back = [g, g * 0.78, g * 0.55];
            b.belly = [0.86, 0.78, 0.6];
            b.wing_col = darker(b.back, 0.22);
            b.cap = b.back;
            b.beak_col = [0.35, 0.28, 0.2];
            b.leg_col = [0.7, 0.6, 0.4];
            b.eye_white = true;
            b.big_eye = true;
            b.face_disk = Some(lighter(b.belly, 0.12));
        }
        Arch::Parrot => {
            b.body = Ell { cx: 16.0, cy: 23.0, rx: 7.0, ry: 7.5 };
            b.head = Ell { cx: 23.0, cy: 13.0, rx: 5.0, ry: 4.8 };
            b.beak_len = 3.5;
            b.beak_half = 2.6;
            b.beak_hook = true;
            b.beak_droop = 0.8;
            b.tail_len = 13.0;
            b.tail_ang = 0.05;
            b.tail_half = 1.8;
            b.wing = Ell { cx: 14.0, cy: 22.0, rx: 5.5, ry: 5.0 };
            b.leg_len = 3.0;
            let h1 = rng.gen::<f32>();
            b.back = hsv(h1, 0.85, 0.85);
            b.belly = hsv(h1, 0.8, 0.7);
            b.wing_col = hsv((h1 + 0.33).fract(), 0.85, 0.8);
            b.cap = hsv((h1 + 0.15).fract(), 0.85, 0.85);
            b.accent = hsv((h1 + 0.5).fract(), 0.9, 0.85);
            b.beak_col = [0.2, 0.18, 0.18];
            b.leg_col = [0.4, 0.4, 0.42];
            if rng.gen_bool(0.4) {
                b.crest = Crest::Big;
            }
        }
        Arch::Penguin => {
            b.body = Ell { cx: 19.0, cy: 23.0, rx: 6.5, ry: 9.5 };
            b.head = Ell { cx: 21.0, cy: 11.0, rx: 4.4, ry: 4.4 };
            b.beak_len = 3.5;
            b.beak_half = 1.4;
            b.beak_droop = 0.3;
            b.tail_len = 3.0;
            b.tail_ang = -0.4;
            b.wing = Ell { cx: 15.5, cy: 24.0, rx: 3.0, ry: 6.0 };
            b.legs = true;
            b.leg_len = 2.0;
            b.leg_x = [18.0, 21.0];
            b.back = [0.13, 0.13, 0.17];
            b.belly = [0.96, 0.96, 0.95];
            b.wing_col = [0.1, 0.1, 0.14];
            b.cap = [0.1, 0.1, 0.14];
            b.beak_col = [0.9, 0.6, 0.15];
            b.leg_col = [0.9, 0.55, 0.15];
            b.accent = [0.98, 0.8, 0.1];
        }
        Arch::Chicken => {
            b.body = Ell { cx: 16.0, cy: 24.0, rx: 8.5, ry: 7.5 };
            b.head = Ell { cx: 24.0, cy: 14.0, rx: 4.4, ry: 4.4 };
            b.neck = Some((21.0, 20.0, 24.0, 15.0, 3.0));
            b.beak_len = 3.0;
            b.beak_half = 1.8;
            b.tail_len = 8.0;
            b.tail_ang = 0.7;
            b.tail_half = 3.2;
            b.wing = Ell { cx: 14.0, cy: 24.0, rx: 5.5, ry: 4.2 };
            b.leg_len = 5.0;
            b.leg_x = [15.0, 18.5];
            b.crest = Crest::Comb;
            b.beak_col = [0.95, 0.72, 0.2];
            b.leg_col = [0.95, 0.7, 0.25];
            b.accent = [0.85, 0.15, 0.12]; // comb/wattle red
            let rooster = rng.gen_bool(0.4);
            if rooster {
                let h = r(rng, 0.02, 0.09);
                b.back = hsv(h, 0.7, 0.45);
                b.belly = hsv(h, 0.6, 0.3);
                b.wing_col = hsv((h + 0.9).fract(), 0.8, 0.35);
                b.cap = darker(b.back, 0.3);
                b.tail_len = 11.0;
            } else {
                let t = rng.gen_range(0..3);
                let c = [[0.62, 0.42, 0.28], [0.95, 0.95, 0.92], [0.2, 0.18, 0.2]][t];
                b.back = c;
                b.belly = lighter(c, 0.2);
                b.wing_col = darker(c, 0.2);
                b.cap = c;
            }
        }
        Arch::Chick => {
            b.body = Ell { cx: 18.0, cy: 25.0, rx: 6.5, ry: 6.0 };
            b.head = Ell { cx: 22.0, cy: 17.0, rx: 5.0, ry: 4.8 };
            b.beak_len = 2.6;
            b.beak_half = 1.6;
            b.beak_droop = 0.2;
            b.tail_len = 3.0;
            b.tail_ang = 0.5;
            b.tail_half = 2.2;
            b.wing = Ell { cx: 16.5, cy: 25.0, rx: 4.0, ry: 3.4 };
            b.legs = true;
            b.leg_len = 3.0;
            b.leg_x = [17.0, 20.0];
            let y = r(rng, 0.13, 0.16);
            b.back = hsv(y, 0.75, 0.98);
            b.belly = hsv(y, 0.5, 1.0);
            b.wing_col = hsv(y, 0.8, 0.9);
            b.cap = b.back;
            b.beak_col = [0.95, 0.6, 0.15];
            b.leg_col = [0.95, 0.6, 0.2];
            b.eye_white = false;
        }
        Arch::Wader => {
            b.body = Ell { cx: 15.0, cy: 21.0, rx: 7.5, ry: 5.0 };
            b.head = Ell { cx: 27.0, cy: 9.0, rx: 3.2, ry: 3.2 };
            b.neck = Some((18.0, 18.0, 27.0, 10.5, 1.8));
            b.beak_len = 6.5;
            b.beak_half = 1.2;
            b.beak_droop = 0.2;
            b.tail_len = 5.0;
            b.tail_ang = 0.1;
            b.wing = Ell { cx: 13.0, cy: 20.0, rx: 5.5, ry: 3.6 };
            b.legs = true;
            b.leg_len = 12.0;
            b.leg_x = [14.0, 17.0];
            let flamingo = rng.gen_bool(0.4);
            if flamingo {
                b.back = hsv(0.96, 0.5, 0.95);
                b.belly = hsv(0.96, 0.35, 1.0);
                b.wing_col = hsv(0.98, 0.6, 0.9);
                b.cap = b.back;
                b.beak_col = [0.2, 0.18, 0.2];
                b.leg_col = hsv(0.96, 0.45, 0.9);
            } else {
                let g = r(rng, 0.55, 0.75);
                b.back = [g, g, g * 1.0];
                b.belly = [0.95, 0.95, 0.96];
                b.wing_col = darker(b.back, 0.3);
                b.cap = darker(b.back, 0.4);
                b.beak_col = [0.95, 0.72, 0.2];
                b.leg_col = [0.85, 0.65, 0.3];
            }
        }
    }
    b.outline = darker(b.back, 0.78);
    b
}

// --------------------------------------------------------------------------
// draw a bird onto a fresh transparent tile
// --------------------------------------------------------------------------
fn draw(bird: &Bird) -> RgbaImage {
    let mut cv = Canvas::new();

    // 1) tail
    {
        let bx = bird.body.cx - bird.body.rx * 0.9;
        let by = bird.body.cy;
        let ang = bird.tail_ang;
        let tip = (bx - bird.tail_len * ang.cos(), by - bird.tail_len * ang.sin());
        let up = (bx, by - bird.tail_half);
        let dn = (bx, by + bird.tail_half);
        tri(&mut cv, [up, dn, tip], bird.wing_col, darker(bird.wing_col, 0.3));
        if bird.tail_fork {
            tri(&mut cv, [up, (bx, by), tip], bird.back, darker(bird.back, 0.3));
        }
    }
    // 2) legs (behind body) — drawn later thin on top; here just skip
    // 3) neck
    if let Some((ax, ay, nx, ny, nr)) = bird.neck {
        capsule(&mut cv, ax, ay, nx, ny, nr, bird.back, darker(bird.back, 0.3));
    }
    // 4) body
    blob(&mut cv, bird.body, bird.back, bird.belly);
    // 5) wing
    blob(&mut cv, bird.wing, bird.wing_col, darker(bird.wing_col, 0.2));
    // wing covert line
    {
        let e = bird.wing;
        for k in 0..3 {
            let yy = (e.cy - 1.0 + k as f32 * 1.6) as i32;
            for xx in (e.cx - e.rx * 0.6) as i32..=(e.cx + e.rx * 0.5) as i32 {
                if cv.is_filled(xx, yy) {
                    cv.put(xx, yy, darker(bird.wing_col, 0.35));
                }
            }
        }
    }
    // 6) head
    blob(&mut cv, bird.head, bird.cap, mix(bird.cap, bird.belly, 0.4));
    // 7) beak
    {
        let hx = bird.head.cx + bird.head.rx * 0.7;
        let hy = bird.head.cy + bird.beak_up;
        let tipx = hx + bird.beak_len;
        let tipy = hy + bird.beak_droop * bird.beak_len * 0.25;
        let up = (hx, hy - bird.beak_half);
        let dn = (hx, hy + bird.beak_half);
        tri(&mut cv, [up, dn, (tipx, tipy)], bird.beak_col, darker(bird.beak_col, 0.3));
        if bird.beak_hook {
            // little downward hook at the tip
            for k in 0..2 {
                cv.put(tipx as i32 - k, (tipy + 1.0) as i32 + k, darker(bird.beak_col, 0.2));
            }
        }
    }

    // ---- outline pass ----
    let mut img = RgbaImage::new(W, H);
    for y in 0..H as i32 {
        for x in 0..W as i32 {
            if cv.is_filled(x, y) {
                img.put_pixel(x as u32, y as u32, to_rgba(cv.col[(y as u32 * W + x as u32) as usize]));
            } else {
                let near = cv.is_filled(x - 1, y)
                    || cv.is_filled(x + 1, y)
                    || cv.is_filled(x, y - 1)
                    || cv.is_filled(x, y + 1)
                    || cv.is_filled(x - 1, y - 1)
                    || cv.is_filled(x + 1, y - 1)
                    || cv.is_filled(x - 1, y + 1)
                    || cv.is_filled(x + 1, y + 1);
                if near {
                    img.put_pixel(x as u32, y as u32, to_rgba(bird.outline));
                }
            }
        }
    }

    // ---- details on top ----
    // legs
    if bird.legs {
        let top = bird.body.cy + bird.body.ry * 0.7;
        let foot_y = top + bird.leg_len;
        for lx in bird.leg_x {
            for yy in top as i32..=foot_y as i32 {
                img.put_pixel(lx as u32, yy as u32, to_rgba(darker(bird.leg_col, 0.15)));
            }
            // foot: forward toes
            for fx in 0..3 {
                img.put_pixel(lx as u32 + fx, foot_y as u32, to_rgba(bird.leg_col));
            }
        }
    }
    // crest / comb / ear tufts
    match bird.crest {
        Crest::Tuft => {
            let hx = bird.head.cx as i32;
            let hy = (bird.head.cy - bird.head.ry) as i32;
            for k in 0..3 {
                img.put_pixel((hx - k) as u32, (hy - k) as u32, to_rgba(darker(bird.cap, 0.1)));
            }
        }
        Crest::Big => {
            let hx = (bird.head.cx - 1.0) as i32;
            let hy = (bird.head.cy - bird.head.ry) as i32;
            for k in 0..5 {
                img.put_pixel((hx - k) as u32, (hy - 1 - k / 2) as u32, to_rgba(bird.accent));
                img.put_pixel((hx - k) as u32, (hy - k / 2) as u32, to_rgba(darker(bird.accent, 0.2)));
            }
        }
        Crest::Comb => {
            let hx = bird.head.cx as i32;
            let hy = (bird.head.cy - bird.head.ry) as i32;
            for k in 0..4 {
                img.put_pixel((hx + k - 1) as u32, (hy - 1) as u32, to_rgba(bird.accent));
                if k % 2 == 0 {
                    img.put_pixel((hx + k - 1) as u32, (hy - 2) as u32, to_rgba(bird.accent));
                }
            }
            // wattle
            img.put_pixel((bird.head.cx + bird.head.rx * 0.4) as u32, (bird.head.cy + bird.head.ry * 0.8) as u32, to_rgba(bird.accent));
        }
        Crest::Ear => {
            // pronounced ear tufts: 5px angled spikes with a dark tip
            let hy = (bird.head.cy - bird.head.ry * 0.85) as i32;
            for (side, sx) in [(-1.0f32, bird.head.cx - bird.head.rx * 0.55), (1.0, bird.head.cx + bird.head.rx * 0.55)] {
                for k in 0..5 {
                    let c = if k >= 3 { bird.outline } else { darker(bird.cap, 0.15) };
                    img.put_pixel((sx + side * (k as f32 * 0.5)) as u32, (hy - k) as u32, to_rgba(c));
                }
            }
        }
        Crest::None => {}
    }
    // owl facial disk (drawn as a pale patch on the front of the face)
    if let Some(fd) = bird.face_disk {
        let fe = Ell { cx: bird.head.cx + 0.5, cy: bird.head.cy + 0.6, rx: bird.head.rx * 0.85, ry: bird.head.ry * 0.85 };
        for y in (fe.cy - fe.ry) as i32..=(fe.cy + fe.ry) as i32 {
            for x in (fe.cx - fe.rx) as i32..=(fe.cx + fe.rx) as i32 {
                let nx = (x as f32 + 0.5 - fe.cx) / fe.rx;
                let ny = (y as f32 + 0.5 - fe.cy) / fe.ry;
                if nx * nx + ny * ny <= 1.0 && cv.is_filled(x, y) {
                    img.put_pixel(x as u32, y as u32, to_rgba(fd));
                }
            }
        }
    }
    // eye(s)
    {
        if bird.big_eye {
            // two big forward eyes: pale ring + dark pupil + glint
            let ey = (bird.head.cy + 0.5) as i32;
            for (i, ex) in [(bird.head.cx - bird.head.rx * 0.30) as i32, (bird.head.cx + bird.head.rx * 0.42) as i32].into_iter().enumerate() {
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        img.put_pixel((ex + dx) as u32, (ey + dy) as u32, to_rgba([0.97, 0.85, 0.35]));
                    }
                }
                img.put_pixel(ex as u32, ey as u32, to_rgba([0.05, 0.05, 0.08]));
                img.put_pixel((ex + if i == 0 { 0 } else { 1 }) as u32, ey as u32, to_rgba([0.05, 0.05, 0.08]));
                img.put_pixel(ex as u32, (ey - 1) as u32, to_rgba([1.0, 1.0, 1.0]));
            }
        } else {
            let ex = (bird.head.cx + bird.head.rx * 0.35) as u32;
            let ey = (bird.head.cy - bird.head.ry * 0.15) as u32;
            if bird.eye_white {
                img.put_pixel(ex, ey, to_rgba([0.98, 0.95, 0.7]));
                img.put_pixel(ex + 1, ey, to_rgba([0.05, 0.05, 0.08]));
            } else {
                img.put_pixel(ex, ey, to_rgba([0.05, 0.05, 0.08]));
                img.put_pixel(ex, ey - 1, to_rgba([0.9, 0.9, 0.95])); // glint
            }
        }
    }
    img
}

fn upscale(img: &RgbaImage, s: u32) -> RgbaImage {
    imageops::resize(img, img.width() * s, img.height() * s, imageops::FilterType::Nearest)
}

fn checker_bg(cell: u32) -> RgbaImage {
    let mut bg = RgbaImage::new(cell, cell);
    for y in 0..cell {
        for x in 0..cell {
            let c = if ((x / 6) + (y / 6)) % 2 == 0 {
                [232u8, 200, 208, 255]
            } else {
                [220, 182, 194, 255]
            };
            bg.put_pixel(x, y, Rgba(c));
        }
    }
    bg
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("out")?;

    let cell = W * SHEET_UP;
    let gutter = 2u32;

    // 10x10 sheet echoing the reference: random archetypes.
    {
        let cols = 10u32;
        let rows = 10u32;
        let mut sheet = RgbaImage::new(cols * (cell + gutter) + gutter, rows * (cell + gutter) + gutter);
        for p in sheet.pixels_mut() {
            *p = Rgba([40, 34, 44, 255]);
        }
        for i in 0..cols * rows {
            let mut rng = StdRng::seed_from_u64(1000 + i as u64);
            let arch = ARCHES[(rng.gen::<u32>() % ARCHES.len() as u32) as usize];
            let bird = draw(&sample(arch, &mut rng));
            let mut tile = checker_bg(cell);
            imageops::overlay(&mut tile, &upscale(&bird, SHEET_UP), 0, 0);
            let x = gutter + (i % cols) * (cell + gutter);
            let y = gutter + (i / cols) * (cell + gutter);
            imageops::overlay(&mut sheet, &tile, x as i64, y as i64);
        }
        sheet.save("out/birds_100.png")?;
        println!("Wrote out/birds_100.png (100 random birds)");
    }

    // archetype sheet: one row per archetype, 8 seeds.
    {
        let cols = 8u32;
        let rows = ARCHES.len() as u32;
        let mut sheet = RgbaImage::new(cols * (cell + gutter) + gutter, rows * (cell + gutter) + gutter);
        for p in sheet.pixels_mut() {
            *p = Rgba([40, 34, 44, 255]);
        }
        for (ri, arch) in ARCHES.iter().enumerate() {
            for c in 0..cols {
                let mut rng = StdRng::seed_from_u64(50 + ri as u64 * 100 + c as u64);
                let bird = draw(&sample(*arch, &mut rng));
                let mut tile = checker_bg(cell);
                imageops::overlay(&mut tile, &upscale(&bird, SHEET_UP), 0, 0);
                let x = gutter + c * (cell + gutter);
                let y = gutter + ri as u32 * (cell + gutter);
                imageops::overlay(&mut sheet, &tile, x as i64, y as i64);
            }
        }
        sheet.save("out/birds_archetypes.png")?;
        println!("Wrote out/birds_archetypes.png (per-archetype rows)");
    }
    Ok(())
}
