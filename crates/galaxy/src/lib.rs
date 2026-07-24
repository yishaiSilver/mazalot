//! galaxy — a procedural, seed-driven **galaxy map** of star systems you can
//! drag around and zoom into, the layer *above* [`solar`].
//!
//! Pure math, zero dependencies. Where `solar` renders one star system, `galaxy`
//! lays out a whole galaxy of them: hundreds of systems placed with spiral-arm
//! structure, wired together by **hyperlanes** into a connected travel graph, and
//! partitioned into faction **regions**. Same seed => the same galaxy, forever.
//!
//! Each node carries a `system_seed` — hand it straight to `solar`'s
//! `System::generate` to fly into that system. The node's star colour on the map
//! is derived with the *same* draw `solar` uses for its central star, so the
//! yellow dot you click is a yellow star when you arrive (see [`sun_kind_of_seed`]).
//!
//! This crate is self-contained by the workspace rule (each "type" crate shares
//! no code with the others — only third-party deps and the manifest). It carries
//! its own compact noise/color/graph primitives.
//!
//! ## Layout (structured procedural, not random soup)
//!   1. **Placement** — best-candidate (Mitchell) blue-noise sampling weighted by
//!      a spiral-arm + core-bulge **density field**, so systems cluster on arms
//!      and thin toward the rim with even local spacing.
//!   2. **Hyperlanes** — a Euclidean **minimum spanning tree** (guarantees the
//!      galaxy is fully traversable) plus a tunable fraction of short
//!      nearest-neighbour edges (loops / alternate routes). One `link_density`
//!      knob spans tree-like-and-chokepointy → dense web.
//!   3. **Regions** — farthest-point faction anchors + nearest-anchor Voronoi, so
//!      territories are contiguous; a core→rim gradient sets how developed each is.
//!
//! ## Render (see [`render_map`])
//!   backdrop (cached, camera-only): galactic **haze** following the arm density,
//!   region-tinted territory wash, faint background dust, and the hyperlane graph
//!   → then per-frame the **system glyphs** (star-coloured, gently twinkling) plus
//!   hover/selection rings. The backdrop is time-independent, so a still camera
//!   memcpys it and only the glyphs re-draw — the same trick `solar` uses.

use std::f32::consts::TAU;

// ===========================================================================
// Noise + math primitives (this crate's own copy — shared with nobody).
// hash3 / Rng are kept BYTE-IDENTICAL to solar so `sun_kind_of_seed` reproduces
// solar's central-star draw; do not "improve" them independently.
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

/// Integer hash — a distinct, deterministic 32-bit value per (a, b). Used to
/// derive each node's `system_seed` from the galaxy seed + node index.
fn mix_u32(a: u32, b: u32) -> u32 {
    let mut h = a ^ b.wrapping_mul(0x9e37_79b1);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^= h >> 16;
    h
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

/// Ordered-dither offset from an 8x8 Bayer matrix, in −0.5..0.5 — kills banding
/// in the faint haze while staying crisp (pixel-art) under pan/zoom.
const BAYER: [u8; 64] = [
    0, 32, 8, 40, 2, 34, 10, 42, 48, 16, 56, 24, 50, 18, 58, 26, 12, 44, 4, 36, 14, 46,
    6, 38, 60, 28, 52, 20, 62, 30, 54, 22, 3, 35, 11, 43, 1, 33, 9, 41, 51, 19, 59, 27,
    49, 17, 57, 25, 15, 47, 7, 39, 13, 45, 5, 37, 63, 31, 55, 23, 61, 29, 53, 21,
];
fn bayer(x: u32, y: u32) -> f32 {
    (BAYER[((y % 8) * 8 + (x % 8)) as usize] as f32 + 0.5) / 64.0 - 0.5
}

/// Tiny deterministic RNG for galaxy generation. **Byte-identical to solar's
/// `Rng`** so [`sun_kind_of_seed`] reproduces the star `solar` would show.
struct Rng {
    seed: i32,
    ctr: i32,
}
impl Rng {
    fn new(seed: u32) -> Rng {
        Rng { seed: seed as i32, ctr: 0 }
    }
    fn f(&mut self) -> f32 {
        self.ctr = self.ctr.wrapping_add(1);
        hash3(self.seed, self.ctr, 0x9e37)
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f()
    }
}

// ===========================================================================
// Star + region tables
// ===========================================================================

/// Star-glyph colours, index-aligned with **solar's `SUNS`** table
/// (yellow dwarf, orange dwarf, red giant, white star, blue giant). Kept in sync
/// with the `SUN_NAMES` array in the web page and solar's own table.
const SUN_TINT: &[Rgb] = &[
    [1.00, 0.90, 0.55], // yellow dwarf
    [1.00, 0.72, 0.42], // orange dwarf
    [1.00, 0.54, 0.42], // red giant
    [0.93, 0.96, 1.00], // white star
    [0.64, 0.80, 1.00], // blue giant
];

/// Reproduce the central-star archetype `solar` picks for `system_seed` — the
/// FIRST draw of solar's `System::generate_n` is
/// `Rng::new(seed ^ 0x5013a1); (rng.f() * SUNS.len()) as usize % SUNS.len()`
/// with `SUNS.len() == 5`. Replicated here (not shared) so the map glyph matches
/// the system you fly into. If solar's star table length changes, change the `5`.
pub fn sun_kind_of_seed(system_seed: u32) -> usize {
    let mut rng = Rng::new(system_seed ^ 0x5013_a1);
    (rng.f() * 5.0) as usize % 5
}

/// A faction region: a name (mirrored in the web `REGION_NAMES` array) and a map
/// tint for its territory wash + capital glyphs.
struct Region {
    name: &'static str,
    tint: Rgb,
}
const REGIONS: &[Region] = &[
    Region { name: "Republic",    tint: [0.40, 0.62, 1.00] },
    Region { name: "Syndicate",   tint: [1.00, 0.66, 0.30] },
    Region { name: "Free Worlds", tint: [0.42, 0.90, 0.72] },
    Region { name: "Verge",       tint: [0.80, 0.46, 0.96] },
    Region { name: "Concord",     tint: [0.98, 0.82, 0.40] },
    Region { name: "Reach",       tint: [0.96, 0.44, 0.56] },
    Region { name: "Expanse",     tint: [0.44, 0.78, 0.98] },
    Region { name: "Drift",       tint: [0.72, 0.74, 0.82] },
];
pub fn region_count_total() -> usize {
    REGIONS.len()
}
pub fn region_name(i: usize) -> &'static str {
    REGIONS[i % REGIONS.len()].name
}
pub fn sun_kind_name_count() -> usize {
    SUN_TINT.len()
}

// ===========================================================================
// Galaxy generation
// ===========================================================================

/// Rough radius of the galactic disc in world units. Node coordinates fall
/// within this; the camera fits it via [`Galaxy::extent`].
const GALAXY_R: f32 = 1000.0;

/// One star system on the map.
#[derive(Clone, Copy)]
pub struct Node {
    pub x: f32,
    pub y: f32,
    pub system_seed: u32, // hand to solar's System::generate
    pub star: u8,         // index into SUN_TINT (== solar sun kind)
    pub region: u8,       // index into REGIONS
    pub degree: u8,       // hyperlane count (hub-ness → glyph size)
    pub importance: f32,  // 0..1, glyph size / label priority
    pub twinkle: f32,     // per-node phase so stars don't pulse in lockstep
}

/// A vivid star-forming nebula cloud placed on a spiral arm (the Kurzgesagt
/// look): a saturated blob of colour washed into the galactic haze.
#[derive(Clone, Copy)]
struct Bloom {
    x: f32,
    y: f32,
    r: f32,
    col: Rgb,
}

/// A generated galaxy: nodes + the hyperlane edge list. Deterministic in `seed`.
/// `node_scale`/`haze` are live view multipliers (do not change identity).
pub struct Galaxy {
    pub seed: u32,
    pub nodes: Vec<Node>,
    pub edges: Vec<(u32, u32)>, // undirected, a < b
    pub arms: u32,
    pub twist: f32, // log-spiral tightness (shared by placement, haze + blooms)
    blooms: Vec<Bloom>,
    pub node_scale: f32,
    pub haze: f32,
    // Cached backdrop (haze + territory wash + dust + hyperlanes) keyed on the
    // camera + view; reused by `render_map_cached` while the camera is still.
    bg_cache: Vec<u8>,
    bg_key: Option<BgKey>,
}

/// Spiral-arm + core-bulge density at world `(x, y)`, in ~[0, 1]. Drives both
/// where systems are placed and how the background haze glows, so the two agree.
fn density(x: f32, y: f32, arms: u32, twist: f32) -> f32 {
    let r = (x * x + y * y).sqrt();
    let rr = r / GALAXY_R;
    // Disc falloff: full in the core, →0 at the rim.
    let radial = smoothstep(1.05, 0.02, rr);
    // Log-spiral arms: cos of (arms·θ − twist·ln r) peaks on the arm ridges.
    let theta = y.atan2(x);
    let phase = theta * arms as f32 - twist * r.max(1.0).ln();
    let ridge = 0.5 + 0.5 * phase.cos();
    let arm = 0.22 + 0.78 * ridge.powf(2.2); // inter-arm keeps a low floor
    // Central bulge ignores the arms (a bright dense core).
    let bulge = smoothstep(0.30, 0.0, rr);
    // Break the arms up so they aren't clean sine ridges.
    let n = fbm(x / 190.0, y / 190.0, 3.7, 3);
    clamp01(radial * arm * (0.5 + 0.95 * n) + bulge * 0.85)
}

impl Galaxy {
    /// Build the galaxy for `seed` with default structure.
    pub fn generate(seed: u32) -> Galaxy {
        Galaxy::generate_params(seed, 0, -1.0, 0)
    }

    /// Build the galaxy for `seed`. `count > 0` forces the system count (else the
    /// seed-derived ~220); `link_density` in [0,1] (negative = default 0.35) sets
    /// how many extra hyperlanes beyond the spanning tree (0 = tree, 1 = dense
    /// web); `arms > 0` forces the spiral-arm count (else 2..=4 by seed).
    pub fn generate_params(seed: u32, count: u32, link_density: f32, arms: u32) -> Galaxy {
        let mut rng = Rng::new(seed ^ 0x9a1c_05);
        let n = if count > 0 {
            (count as usize).clamp(24, 600)
        } else {
            180 + (rng.f() * 90.0) as usize // ~180..=270
        };
        let link_density = if link_density < 0.0 { 0.35 } else { link_density.clamp(0.0, 1.0) };
        let arms = if arms > 0 { arms.clamp(1, 6) } else { 2 + (rng.f() * 3.0) as u32 }; // 2..=4
        let twist = rng.range(1.8, 2.8);

        // --- 1. placement: Mitchell best-candidate weighted by the density field.
        let mut pos: Vec<(f32, f32)> = Vec::with_capacity(n);
        let k = 14; // candidates per point
        for i in 0..n {
            let mut best = (0.0f32, 0.0f32);
            let mut best_score = -1.0f32;
            for _ in 0..k {
                // Uniform sample in the disc (r = R·√u for equal area).
                let u = rng.f();
                let rad = GALAXY_R * u.sqrt();
                let ang = rng.f() * TAU;
                let (cx, cy) = (rad * ang.cos(), rad * ang.sin());
                let dens = density(cx, cy, arms, twist);
                // Distance to the nearest already-placed system.
                let mut md = f32::MAX;
                for &(px, py) in &pos {
                    let d = (px - cx) * (px - cx) + (py - cy) * (py - cy);
                    if d < md {
                        md = d;
                    }
                }
                let md = if i == 0 { GALAXY_R } else { md.sqrt() };
                // Prefer well-spaced candidates that also sit in dense regions.
                let score = md * (0.2 + dens);
                if score > best_score {
                    best_score = score;
                    best = (cx, cy);
                }
            }
            pos.push(best);
        }

        // --- 2. hyperlanes: Euclidean MST (Prim) + a fraction of short kNN edges.
        let mut adj = vec![false; n * n]; // dedup / membership (no hashing in wasm)
        let mut edges: Vec<(u32, u32)> = Vec::new();
        let dist2 = |a: usize, b: usize| {
            let (ax, ay) = pos[a];
            let (bx, by) = pos[b];
            (ax - bx) * (ax - bx) + (ay - by) * (ay - by)
        };
        let add_edge = |edges: &mut Vec<(u32, u32)>, adj: &mut Vec<bool>, a: usize, b: usize| {
            if a == b {
                return;
            }
            let (lo, hi) = (a.min(b), a.max(b));
            if !adj[lo * n + hi] {
                adj[lo * n + hi] = true;
                edges.push((lo as u32, hi as u32));
            }
        };
        // Prim's MST from node 0.
        if n > 1 {
            let mut in_tree = vec![false; n];
            let mut mind = vec![f32::MAX; n];
            let mut parent = vec![usize::MAX; n];
            mind[0] = 0.0;
            for _ in 0..n {
                // Cheapest fringe node not yet in the tree.
                let mut u = usize::MAX;
                let mut best = f32::MAX;
                for v in 0..n {
                    if !in_tree[v] && mind[v] < best {
                        best = mind[v];
                        u = v;
                    }
                }
                if u == usize::MAX {
                    break;
                }
                in_tree[u] = true;
                if parent[u] != usize::MAX {
                    add_edge(&mut edges, &mut adj, u, parent[u]);
                }
                for v in 0..n {
                    if !in_tree[v] {
                        let d = dist2(u, v);
                        if d < mind[v] {
                            mind[v] = d;
                            parent[v] = u;
                        }
                    }
                }
            }
        }
        // Extra short edges: connect each node to a few nearest neighbours with
        // probability `link_density`, capped in length so no long ugly crossings.
        let cap2 = (GALAXY_R * 0.16).powi(2);
        let knn = 5usize;
        let mut cand: Vec<(f32, usize)> = Vec::with_capacity(n);
        for a in 0..n {
            cand.clear();
            for b in 0..n {
                if a != b {
                    cand.push((dist2(a, b), b));
                }
            }
            // Partial sort: pull the knn nearest to the front.
            let m = knn.min(cand.len());
            cand.select_nth_unstable_by(m.saturating_sub(1).max(0), |x, y| {
                x.0.partial_cmp(&y.0).unwrap_or(std::cmp::Ordering::Equal)
            });
            for &(d2, b) in cand.iter().take(m) {
                if d2 <= cap2 && rng.f() < link_density {
                    add_edge(&mut edges, &mut adj, a, b);
                }
            }
        }

        // --- 3. regions: farthest-point anchors (spread) + a couple of Lloyd
        // relaxation passes so faction territories are contiguous AND reasonably
        // even in size, rather than one central Voronoi cell swallowing the disc.
        let region_n = (3 + (rng.f() * (REGIONS.len() as f32 - 2.0)) as usize).clamp(3, REGIONS.len());
        let mut anchors: Vec<usize> = Vec::with_capacity(region_n);
        // First anchor: the node farthest from the galactic centre (an extreme
        // point); farthest-point sampling then spreads the rest around the disc.
        {
            let mut best = 0usize;
            let mut bestd = -1.0f32;
            for i in 0..n {
                let d = pos[i].0 * pos[i].0 + pos[i].1 * pos[i].1;
                if d > bestd {
                    bestd = d;
                    best = i;
                }
            }
            anchors.push(best);
        }
        while anchors.len() < region_n {
            // Add the node farthest (in min-distance) from the current anchors.
            let mut best = 0usize;
            let mut best_min = -1.0f32;
            for i in 0..n {
                let mut md = f32::MAX;
                for &a in &anchors {
                    md = md.min(dist2(i, a));
                }
                if md > best_min {
                    best_min = md;
                    best = i;
                }
            }
            anchors.push(best);
        }
        // Lloyd relaxation: reassign nodes to the nearest anchor, then move each
        // anchor to the node nearest its cluster centroid. Evens out territory
        // areas while keeping them a single connected blob each.
        for _ in 0..2 {
            let mut sum = vec![(0.0f32, 0.0f32, 0usize); anchors.len()];
            for i in 0..n {
                let mut which = 0usize;
                let mut bd = f32::MAX;
                for (ai, &a) in anchors.iter().enumerate() {
                    let d = dist2(i, a);
                    if d < bd {
                        bd = d;
                        which = ai;
                    }
                }
                sum[which].0 += pos[i].0;
                sum[which].1 += pos[i].1;
                sum[which].2 += 1;
            }
            for (ai, s) in sum.iter().enumerate() {
                if s.2 == 0 {
                    continue;
                }
                let (cx, cy) = (s.0 / s.2 as f32, s.1 / s.2 as f32);
                let mut best = anchors[ai];
                let mut bd = f32::MAX;
                for i in 0..n {
                    let d = (pos[i].0 - cx).powi(2) + (pos[i].1 - cy).powi(2);
                    if d < bd {
                        bd = d;
                        best = i;
                    }
                }
                anchors[ai] = best;
            }
        }
        // Assign each node to its nearest anchor; map anchor→region index/tint by
        // radius so the innermost anchor is region 0 (the developed core).
        let mut anchor_r: Vec<(f32, usize)> = anchors
            .iter()
            .enumerate()
            .map(|(idx, &a)| (pos[a].0 * pos[a].0 + pos[a].1 * pos[a].1, idx))
            .collect();
        anchor_r.sort_by(|p, q| p.0.partial_cmp(&q.0).unwrap_or(std::cmp::Ordering::Equal));
        let mut anchor_region = vec![0u8; anchors.len()];
        for (region_idx, &(_, anchor_idx)) in anchor_r.iter().enumerate() {
            anchor_region[anchor_idx] = (region_idx % REGIONS.len()) as u8;
        }

        // --- degree counts (hub-ness) from the finished edge list.
        let mut degree = vec![0u16; n];
        for &(a, b) in &edges {
            degree[a as usize] = degree[a as usize].saturating_add(1);
            degree[b as usize] = degree[b as usize].saturating_add(1);
        }

        // --- assemble nodes.
        let mut nodes = Vec::with_capacity(n);
        for i in 0..n {
            let (x, y) = pos[i];
            // Nearest anchor → region.
            let mut which = 0usize;
            let mut bd = f32::MAX;
            for (ai, &a) in anchors.iter().enumerate() {
                let d = dist2(i, a);
                if d < bd {
                    bd = d;
                    which = ai;
                }
            }
            let region = anchor_region[which];
            let system_seed = mix_u32(seed, (i as u32).wrapping_mul(2_654_435_761).wrapping_add(1));
            let star = sun_kind_of_seed(system_seed) as u8;
            let deg = degree[i].min(255) as u8;
            let rr = (x * x + y * y).sqrt() / GALAXY_R;
            // Importance: hubs and core systems read as bigger, brighter stars.
            let is_capital = anchors.iter().any(|&a| a == i);
            let importance = clamp01(
                0.28 + 0.14 * deg as f32 + 0.30 * (1.0 - rr) + if is_capital { 0.5 } else { 0.0 }
                    + 0.10 * hash3(i as i32, 7, 3),
            );
            let twinkle = hash3(i as i32, 11, 5) * TAU;
            nodes.push(Node {
                x,
                y,
                system_seed,
                star,
                region,
                degree: deg,
                importance,
                twinkle,
            });
        }

        // --- star-forming nebula blooms: vivid clouds placed ON the arms. Solve
        // the log-spiral for a point on an arm ridge at a random radius, jitter
        // it, and give it a saturated tint. These are the pink/cyan pockets.
        const NEB: &[Rgb] = &[
            [0.92, 0.26, 0.66], // magenta
            [0.98, 0.44, 0.60], // pink
            [0.26, 0.82, 0.92], // cyan
            [0.60, 0.34, 0.92], // violet
            [0.96, 0.32, 0.44], // rose
            [0.22, 0.80, 0.72], // teal
        ];
        let bloom_n = 12 + (rng.f() * 8.0) as usize; // 12..=19
        let mut blooms = Vec::with_capacity(bloom_n);
        for _ in 0..bloom_n {
            let r = rng.range(0.18, 0.95) * GALAXY_R;
            let arm_k = (rng.f() * arms as f32) as u32;
            // theta on an arm ridge: arms·θ − twist·ln r ≡ 2π·k  (+ jitter).
            let theta =
                (TAU * arm_k as f32 + twist * r.max(1.0).ln()) / arms as f32 + rng.range(-0.18, 0.18);
            let (s, c) = theta.sin_cos();
            let col = NEB[(rng.f() * NEB.len() as f32) as usize % NEB.len()];
            blooms.push(Bloom {
                x: c * r + rng.range(-40.0, 40.0),
                y: s * r + rng.range(-40.0, 40.0),
                r: rng.range(70.0, 190.0),
                col,
            });
        }

        Galaxy {
            seed,
            nodes,
            edges,
            arms,
            twist,
            blooms,
            node_scale: 1.0,
            haze: 1.0,
            bg_cache: Vec::new(),
            bg_key: None,
        }
    }

    /// Live view multipliers: glyph size scale and haze intensity (0 = off).
    pub fn set_view(&mut self, node_scale: f32, haze: f32) {
        self.node_scale = node_scale.clamp(0.2, 4.0);
        self.haze = haze.clamp(0.0, 3.0);
    }

    /// Farthest node radius from the galactic centre (+margin) — for fit-zoom.
    pub fn extent(&self) -> f32 {
        let mut r: f32 = GALAXY_R * 0.2;
        for nd in &self.nodes {
            r = r.max((nd.x * nd.x + nd.y * nd.y).sqrt());
        }
        r + 60.0
    }
}

// ===========================================================================
// Camera + world→screen (own copy; identical idiom to solar)
// ===========================================================================

/// Camera over the galaxy. `x,y` is the world point at the viewport centre;
/// `zoom` scales world units to pixels.
#[derive(Clone, Copy)]
pub struct Camera {
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
}
impl Camera {
    pub fn centered() -> Camera {
        Camera { x: 0.0, y: 0.0, zoom: 0.3 }
    }
}

#[inline]
fn to_screen(wx: f32, wy: f32, cam: &Camera, w: u32, h: u32) -> (f32, f32) {
    (
        w as f32 * 0.5 + (wx - cam.x) * cam.zoom,
        h as f32 * 0.5 + (wy - cam.y) * cam.zoom,
    )
}
#[inline]
fn to_world(sx: f32, sy: f32, cam: &Camera, w: u32, h: u32) -> (f32, f32) {
    (
        cam.x + (sx - w as f32 * 0.5) / cam.zoom,
        cam.y + (sy - h as f32 * 0.5) / cam.zoom,
    )
}

// ===========================================================================
// Blend helpers
// ===========================================================================

/// Additively blend `col * a` onto the pixel at `(x, y)` (bounds-checked),
/// leaving the frame opaque. The map is built up from faint additive layers.
#[inline]
fn add_px(out: &mut [u8], w: u32, h: u32, x: i32, y: i32, col: Rgb, a: f32) {
    if x < 0 || y < 0 || x >= w as i32 || y >= h as i32 || a <= 0.0 {
        return;
    }
    let idx = ((y as u32 * w + x as u32) * 4) as usize;
    let r = clamp01(out[idx] as f32 / 255.0 + col[0] * a);
    let g = clamp01(out[idx + 1] as f32 / 255.0 + col[1] * a);
    let b = clamp01(out[idx + 2] as f32 / 255.0 + col[2] * a);
    out[idx] = (r * 255.0) as u8;
    out[idx + 1] = (g * 255.0) as u8;
    out[idx + 2] = (b * 255.0) as u8;
    out[idx + 3] = 255;
}

/// A soft additive disc (star glow / territory splat) centred at screen `(sx,sy)`.
fn splat(out: &mut [u8], w: u32, h: u32, sx: f32, sy: f32, rad: f32, col: Rgb, bright: f32, falloff_pow: f32) {
    if rad <= 0.0 || bright <= 0.0 {
        return;
    }
    let r = rad.ceil() as i32;
    let (cx, cy) = (sx, sy);
    let x0 = (sx as i32) - r;
    let x1 = (sx as i32) + r;
    let y0 = (sy as i32) - r;
    let y1 = (sy as i32) + r;
    let inv = 1.0 / rad;
    for py in y0..=y1 {
        for px in x0..=x1 {
            let d = ((px as f32 + 0.5 - cx).powi(2) + (py as f32 + 0.5 - cy).powi(2)).sqrt() * inv;
            if d >= 1.0 {
                continue;
            }
            let f = (1.0 - d).powf(falloff_pow);
            add_px(out, w, h, px, py, col, bright * f);
        }
    }
}

/// A thin additive ring outline (hover / selection highlight).
fn ring(out: &mut [u8], w: u32, h: u32, sx: f32, sy: f32, rad: f32, col: Rgb, bright: f32) {
    let steps = ((TAU * rad).ceil() as i32).max(24);
    for k in 0..steps {
        let a = TAU * k as f32 / steps as f32;
        let (s, c) = a.sin_cos();
        let px = (sx + c * rad) as i32;
        let py = (sy + s * rad) as i32;
        add_px(out, w, h, px, py, col, bright);
    }
}

/// A faint additive line between two screen points (DDA) — a hyperlane.
fn line(out: &mut [u8], w: u32, h: u32, ax: f32, ay: f32, bx: f32, by: f32, col: Rgb, bright: f32) {
    let dx = bx - ax;
    let dy = by - ay;
    let steps = (dx.abs().max(dy.abs())).ceil() as i32;
    if steps <= 0 {
        add_px(out, w, h, ax as i32, ay as i32, col, bright);
        return;
    }
    let (sx, sy) = (dx / steps as f32, dy / steps as f32);
    let (mut x, mut y) = (ax, ay);
    for _ in 0..=steps {
        add_px(out, w, h, x as i32, y as i32, col, bright);
        x += sx;
        y += sy;
    }
}

// ===========================================================================
// Backdrop: haze + territory wash + dust + hyperlanes (time-independent)
// ===========================================================================

/// Galactic haze colour by fractional radius: a bright warm-gold core bulge →
/// cool blue arms → teal rim (the Kurzgesagt Milky-Way palette).
fn haze_tint(rr: f32) -> Rgb {
    let core = [1.00, 0.80, 0.40]; // warm gold bulge
    let mid = [0.22, 0.46, 0.86]; // blue arms
    let rim = [0.28, 0.62, 0.74]; // teal outer
    if rr < 0.34 {
        mix(core, mid, rr / 0.34)
    } else {
        mix(mid, rim, (rr - 0.34) / 0.66)
    }
}

fn paint_backdrop(out: &mut [u8], w: u32, h: u32, cam: &Camera, gal: &Galaxy) {
    let z = cam.zoom;
    let arms = gal.arms;
    let twist = gal.twist; // the SAME tightness the systems were placed with

    // --- low-res haze buffer (8px cells → pixel-art clouds), world-transformed.
    const CELL: u32 = 8;
    let nw = (w + CELL - 1) / CELL;
    let nh = (h + CELL - 1) / CELL;
    let mut haze: Vec<[f32; 3]> = vec![[0.0; 3]; (nw * nh) as usize];
    if gal.haze > 0.01 {
        for cy in 0..nh {
            for cx in 0..nw {
                let (sxp, syp) = ((cx * CELL + CELL / 2) as f32, (cy * CELL + CELL / 2) as f32);
                let (wx, wy) = to_world(sxp, syp, cam, w, h);
                let dens = density(wx, wy, arms, twist);
                if dens > 0.01 {
                    let rr = (wx * wx + wy * wy).sqrt() / GALAXY_R;
                    let col = haze_tint(rr.min(1.0));
                    // Contrast curve (dens^1.4) darkens the inter-arm gaps so the
                    // spiral arms read boldly rather than as an even wash.
                    let k = dens.powf(1.4) * gal.haze * 0.44;
                    let c = &mut haze[(cy * nw + cx) as usize];
                    c[0] += col[0] * k;
                    c[1] += col[1] * k;
                    c[2] += col[2] * k;
                }
            }
        }
        // --- territory wash: splat each node's region tint into the haze buffer
        // so faction space reads as a soft colour field. Influence radius grows
        // with zoom (constant world size), clamped so it stays cheap.
        let infl_px = (90.0 * z).clamp(CELL as f32, 240.0);
        let infl_c = (infl_px / CELL as f32).ceil() as i32;
        for nd in &gal.nodes {
            let (sx, sy) = to_screen(nd.x, nd.y, cam, w, h);
            if sx < -infl_px || sy < -infl_px || sx > w as f32 + infl_px || sy > h as f32 + infl_px {
                continue;
            }
            let (ccx, ccy) = ((sx / CELL as f32) as i32, (sy / CELL as f32) as i32);
            let tint = REGIONS[nd.region as usize % REGIONS.len()].tint;
            let inv = CELL as f32 / infl_px;
            for dy in -infl_c..=infl_c {
                for dx in -infl_c..=infl_c {
                    let (gx, gy) = (ccx + dx, ccy + dy);
                    if gx < 0 || gy < 0 || gx >= nw as i32 || gy >= nh as i32 {
                        continue;
                    }
                    let d = ((dx * dx + dy * dy) as f32).sqrt() * inv;
                    if d >= 1.0 {
                        continue;
                    }
                    let f = (1.0 - d) * (1.0 - d) * 0.05 * gal.haze;
                    let c = &mut haze[(gy as u32 * nw + gx as u32) as usize];
                    c[0] += tint[0] * f;
                    c[1] += tint[1] * f;
                    c[2] += tint[2] * f;
                }
            }
        }
        // --- star-forming nebula blooms: vivid, saturated colour pockets on the
        // arms (magenta / pink / cyan). Splatted brighter than the base haze so
        // they pop, but still under the star glyphs. Radius scales with zoom.
        for bl in &gal.blooms {
            let (sx, sy) = to_screen(bl.x, bl.y, cam, w, h);
            let rpx = (bl.r * z).max(CELL as f32);
            if sx < -rpx || sy < -rpx || sx > w as f32 + rpx || sy > h as f32 + rpx {
                continue;
            }
            let (ccx, ccy) = ((sx / CELL as f32) as i32, (sy / CELL as f32) as i32);
            let rc = (rpx / CELL as f32).ceil() as i32;
            let inv = CELL as f32 / rpx;
            for dy in -rc..=rc {
                for dx in -rc..=rc {
                    let (gx, gy) = (ccx + dx, ccy + dy);
                    if gx < 0 || gy < 0 || gx >= nw as i32 || gy >= nh as i32 {
                        continue;
                    }
                    let d = ((dx * dx + dy * dy) as f32).sqrt() * inv;
                    if d >= 1.0 {
                        continue;
                    }
                    let f = (1.0 - d).powi(2) * 0.5 * gal.haze;
                    let c = &mut haze[(gy as u32 * nw + gx as u32) as usize];
                    c[0] += bl.col[0] * f;
                    c[1] += bl.col[1] * f;
                    c[2] += bl.col[2] * f;
                }
            }
        }
    }

    // --- base fill + haze upsample with dither.
    for iy in 0..h {
        let nrow = (iy / CELL) * nw;
        for ix in 0..w {
            let (mut r, mut g, mut b) = (0.010f32, 0.012, 0.030); // deep near-black
            let c = haze[(nrow + ix / CELL) as usize];
            let d = bayer(ix, iy) * 0.012;
            r += (c[0] + d).max(0.0);
            g += (c[1] + d).max(0.0);
            b += (c[2] + d).max(0.0);
            let idx = ((iy * w + ix) * 4) as usize;
            out[idx] = (clamp01(r) * 255.0) as u8;
            out[idx + 1] = (clamp01(g) * 255.0) as u8;
            out[idx + 2] = (clamp01(b) * 255.0) as u8;
            out[idx + 3] = 255;
        }
    }

    // --- faint background dust: hashed points on a world grid, so they belong to
    // the galaxy (pan with it) and thin out as you zoom (constant world density).
    let dust_sp = 46.0f32; // world units between candidate dust cells
    let (w0, h0) = to_world(0.0, 0.0, cam, w, h);
    let (w1, h1) = to_world(w as f32, h as f32, cam, w, h);
    let (cx0, cx1) = ((w0.min(w1) / dust_sp).floor() as i32 - 1, (w0.max(w1) / dust_sp).floor() as i32 + 1);
    let (cy0, cy1) = ((h0.min(h1) / dust_sp).floor() as i32 - 1, (h0.max(h1) / dust_sp).floor() as i32 + 1);
    if (cx1 - cx0) as i64 * (cy1 - cy0) as i64 <= 400_000 {
        for cy in cy0..=cy1 {
            for cx in cx0..=cx1 {
                let hh = hash3(cx, cy, 91);
                if hh < 0.80 {
                    continue;
                }
                let jx = (hh * 137.0).fract();
                let jy = (hh * 71.3 + 0.37).fract();
                let (wx, wy) = ((cx as f32 + jx) * dust_sp, (cy as f32 + jy) * dust_sp);
                let (sx, sy) = to_screen(wx, wy, cam, w, h);
                let s = 0.12 + 0.28 * (hh - 0.80) / 0.20;
                add_px(out, w, h, sx as i32, sy as i32, [0.78, 0.84, 1.0], s);
            }
        }
    }

    // --- hyperlanes: faint additive lines. Skip edges fully off-screen.
    let (wf, hf) = (w as f32, h as f32);
    for &(a, b) in &gal.edges {
        let na = &gal.nodes[a as usize];
        let nb = &gal.nodes[b as usize];
        let (ax, ay) = to_screen(na.x, na.y, cam, w, h);
        let (bx, by) = to_screen(nb.x, nb.y, cam, w, h);
        let minx = ax.min(bx);
        let maxx = ax.max(bx);
        let miny = ay.min(by);
        let maxy = ay.max(by);
        if maxx < 0.0 || minx > wf || maxy < 0.0 || miny > hf {
            continue;
        }
        line(out, w, h, ax, ay, bx, by, [0.34, 0.40, 0.62], 0.16);
    }
}

// ===========================================================================
// Foreground: system glyphs + rings (per frame; twinkle + selection)
// ===========================================================================

fn draw_glyphs(out: &mut [u8], w: u32, h: u32, cam: &Camera, gal: &Galaxy, t: f32, sel: i32, hover: i32) {
    let (wf, hf) = (w as f32, h as f32);
    for (i, nd) in gal.nodes.iter().enumerate() {
        let (sx, sy) = to_screen(nd.x, nd.y, cam, w, h);
        // Glyph size is mostly constant on screen (a star map), nudged by
        // importance and a touch by zoom so hubs pop when you zoom in.
        let core = (1.1 + 2.4 * nd.importance) * gal.node_scale * (0.85 + 0.25 * smoothstep(0.2, 2.0, cam.zoom));
        let glow = core * 3.2;
        if sx + glow < 0.0 || sy + glow < 0.0 || sx - glow > wf || sy - glow > hf {
            continue;
        }
        let tint = SUN_TINT[nd.star as usize % SUN_TINT.len()];
        // Gentle twinkle (never fully off).
        let tw = 0.82 + 0.18 * (t * 1.7 + nd.twinkle).sin();
        // Coloured halo.
        splat(out, w, h, sx, sy, glow, tint, 0.22 * tw, 1.6);
        // Bright core → near-white centre for a crisp star.
        splat(out, w, h, sx, sy, core, tint, 1.0 * tw, 1.2);
        splat(out, w, h, sx, sy, core * 0.5, [1.0, 1.0, 1.0], 0.95 * tw, 1.0);
        // Diffraction glint on the brightest systems — a subtle 4-point star.
        if nd.importance > 0.55 {
            let gl = (core * 2.6).min(22.0);
            let n = gl as i32;
            for k in 1..=n {
                let f = (1.0 - k as f32 / gl).max(0.0).powi(2) * 0.5 * tw;
                add_px(out, w, h, (sx + k as f32) as i32, sy as i32, tint, f);
                add_px(out, w, h, (sx - k as f32) as i32, sy as i32, tint, f);
                add_px(out, w, h, sx as i32, (sy + k as f32) as i32, tint, f);
                add_px(out, w, h, sx as i32, (sy - k as f32) as i32, tint, f);
            }
        }

        if i as i32 == hover && hover != sel {
            ring(out, w, h, sx, sy, glow + 3.0, [0.75, 0.82, 0.95], 0.5);
        }
        if i as i32 == sel {
            let pr = glow + 4.0 + 1.5 * (t * 3.0).sin();
            ring(out, w, h, sx, sy, pr, [1.0, 0.95, 0.7], 0.95);
            ring(out, w, h, sx, sy, pr + 2.0, [1.0, 0.95, 0.7], 0.35);
        }
    }
}

// ===========================================================================
// Public render entry points
// ===========================================================================

/// Render the galaxy map into `out` (RGBA, `w*h*4` bytes). `t` drives the star
/// twinkle + selection pulse; `sel`/`hover` are node indices to highlight (−1 =
/// none). Draw order: backdrop (haze/wash/dust/hyperlanes) → system glyphs.
#[allow(clippy::too_many_arguments)]
pub fn render_map(gal: &Galaxy, w: u32, h: u32, cam: &Camera, t: f32, sel: i32, hover: i32, out: &mut [u8]) {
    assert!(out.len() >= (w * h * 4) as usize);
    paint_backdrop(out, w, h, cam, gal);
    draw_glyphs(out, w, h, cam, gal, t, sel, hover);
}

/// Cache key for the backdrop: fully determined by camera + view (NO time, NO
/// selection), so a still camera reuses it byte-for-byte.
type BgKey = [f32; 7];
fn bg_key(gal: &Galaxy, w: u32, h: u32, cam: &Camera) -> BgKey {
    [w as f32, h as f32, cam.x, cam.y, cam.zoom, gal.haze, gal.seed as f32]
}

/// Like [`render_map`] but caches the (time-independent) backdrop on the galaxy.
/// On a still camera the backdrop is a memcpy and only the twinkling glyphs +
/// selection ring re-draw. Any pan/zoom/view change repaints it once.
#[allow(clippy::too_many_arguments)]
pub fn render_map_cached(gal: &mut Galaxy, w: u32, h: u32, cam: &Camera, t: f32, sel: i32, hover: i32, out: &mut [u8]) {
    let len = (w * h * 4) as usize;
    assert!(out.len() >= len);
    let key = bg_key(gal, w, h, cam);
    if gal.bg_key == Some(key) && gal.bg_cache.len() == len {
        out[..len].copy_from_slice(&gal.bg_cache);
    } else {
        paint_backdrop(out, w, h, cam, gal);
        gal.bg_cache.clear();
        gal.bg_cache.extend_from_slice(&out[..len]);
        gal.bg_key = Some(key);
    }
    draw_glyphs(out, w, h, cam, gal, t, sel, hover);
}

/// Index of the system nearest world point `(wx, wy)` whose glyph is within a
/// screen-space pick tolerance (~18 px scaled a little by importance), or −1.
pub fn node_at(gal: &Galaxy, cam: &Camera, wx: f32, wy: f32) -> i32 {
    let mut best = -1i32;
    let mut best_d = f32::MAX;
    for (i, nd) in gal.nodes.iter().enumerate() {
        let d = (nd.x - wx).powi(2) + (nd.y - wy).powi(2);
        // Screen-space tolerance so picking feels consistent at any zoom.
        let tol_world = (16.0 + 10.0 * nd.importance) / cam.zoom.max(1e-4);
        if d < best_d && d < tol_world * tol_world {
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

    /// Union-Find helper to check the hyperlane graph is fully connected.
    fn connected(n: usize, edges: &[(u32, u32)]) -> bool {
        let mut parent: Vec<usize> = (0..n).collect();
        fn find(p: &mut Vec<usize>, x: usize) -> usize {
            let mut x = x;
            while p[x] != x {
                p[x] = p[p[x]];
                x = p[x];
            }
            x
        }
        for &(a, b) in edges {
            let (ra, rb) = (find(&mut parent, a as usize), find(&mut parent, b as usize));
            parent[ra] = rb;
        }
        let root = find(&mut parent, 0);
        (0..n).all(|i| find(&mut parent, i) == root)
    }

    #[test]
    fn deterministic() {
        let a = Galaxy::generate(7);
        let b = Galaxy::generate(7);
        assert_eq!(a.nodes.len(), b.nodes.len());
        assert_eq!(a.edges.len(), b.edges.len());
        for (na, nb) in a.nodes.iter().zip(&b.nodes) {
            assert_eq!(na.system_seed, nb.system_seed);
            assert_eq!(na.x.to_bits(), nb.x.to_bits());
            assert_eq!(na.star, nb.star);
        }
    }

    #[test]
    fn seeds_are_distinct() {
        let g = Galaxy::generate(7);
        let mut seen = std::collections::HashSet::new();
        for nd in &g.nodes {
            assert!(seen.insert(nd.system_seed), "duplicate system seed");
        }
    }

    #[test]
    fn graph_is_connected() {
        for seed in [1u32, 7, 42, 1000, 999_999] {
            let g = Galaxy::generate(seed);
            assert!(connected(g.nodes.len(), &g.edges), "galaxy {seed} not fully connected");
        }
    }

    #[test]
    fn edges_are_normalized_and_unique() {
        let g = Galaxy::generate(42);
        let mut seen = std::collections::HashSet::new();
        for &(a, b) in &g.edges {
            assert!(a < b, "edge not normalized");
            assert!(seen.insert((a, b)), "duplicate edge");
        }
    }

    #[test]
    fn star_glyph_matches_solar_draw() {
        // The map glyph's star index must equal solar's own first draw for that
        // seed. We can't call solar here (disjoint crate), but we replicate its
        // formula; this pins the contract so a change to one side fails loudly.
        let seed = 123_456u32;
        let mut rng = Rng::new(seed ^ 0x5013_a1);
        let expect = (rng.f() * 5.0) as usize % 5;
        assert_eq!(sun_kind_of_seed(seed), expect);
    }

    #[test]
    fn count_and_arms_overrides() {
        let g = Galaxy::generate_params(3, 300, 0.0, 4);
        assert_eq!(g.nodes.len(), 300);
        assert_eq!(g.arms, 4);
        // link_density 0 → spanning tree only: exactly n-1 edges, still connected.
        assert_eq!(g.edges.len(), 299);
        assert!(connected(300, &g.edges));
    }
}
