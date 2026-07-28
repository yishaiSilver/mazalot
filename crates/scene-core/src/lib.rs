//! scene-core — the shared scene-compositor primitives used by the crates that
//! render a *scene* (a camera over many bodies): solar, moon, comet, asteroid.
//!
//! Everything here was previously copy-pasted (byte for byte) into those
//! crates: the draggable [`Camera`] + [`to_screen`] transform, the seeded
//! [`Rng`], the [`Tile`] render target + [`blit`] alpha compositor, and the
//! [`ORBIT_FLATTEN`] tilt constant. Values are unchanged, so output is
//! identical. Only depends on `noise-core` (for the RNG's hash).

use noise_core::hash3;

/// Vertical squash applied to orbits so a top-down plane reads as a tilted
/// ellipse (1.0 = circle, 0.42 = the shared house look).
pub const ORBIT_FLATTEN: f32 = 0.42;

// ---------------------------------------------------------------------------
// Camera
// ---------------------------------------------------------------------------

/// A draggable 2D camera: `(x, y)` is the world point at the viewport centre;
/// `zoom` scales world units to pixels (1.0 = 1:1).
#[derive(Clone, Copy)]
pub struct Camera {
    pub x: f32,
    pub y: f32,
    pub zoom: f32,
}
impl Camera {
    pub fn centered() -> Camera {
        Camera { x: 0.0, y: 0.0, zoom: 1.0 }
    }
}

/// World → screen for the given viewport.
#[inline]
pub fn to_screen(wx: f32, wy: f32, cam: &Camera, w: u32, h: u32) -> (f32, f32) {
    (
        w as f32 * 0.5 + (wx - cam.x) * cam.zoom,
        h as f32 * 0.5 + (wy - cam.y) * cam.zoom,
    )
}

// ---------------------------------------------------------------------------
// Seeded RNG (SplitMix-ish over hash3)
// ---------------------------------------------------------------------------

/// Tiny deterministic RNG for scene generation. Same seed => same scene.
pub struct Rng {
    pub seed: i32,
    pub ctr: i32,
}
impl Rng {
    pub fn new(seed: u32) -> Rng {
        Rng { seed: seed as i32, ctr: 0 }
    }
    pub fn f(&mut self) -> f32 {
        self.ctr = self.ctr.wrapping_add(1);
        hash3(self.seed, self.ctr, 0x9e37)
    }
    /// Uniform in [lo, hi).
    pub fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f()
    }
    pub fn below(&mut self, p: f32) -> bool {
        self.f() < p
    }
}

// ---------------------------------------------------------------------------
// Tile render target + alpha-blend blit
// ---------------------------------------------------------------------------

/// A rendered body ready to blit: RGBA pixels + its tile size. Alpha is 0
/// off-body, 255 on the opaque disc, and partial in soft halos (e.g. a corona).
///
/// [`Default`] is an empty tile — the starting point for a scene that keeps one
/// around and lets each body renderer resize it, rather than allocating a fresh
/// one per body per frame.
#[derive(Default)]
pub struct Tile {
    pub px: Vec<u8>,
    pub size: u32,
}

impl Tile {
    /// Resize to `size`×`size`, reusing the allocation when it already is.
    ///
    /// Every body renderer that writes into a caller-owned tile starts here, so
    /// that "does this buffer already fit?" is decided in one place rather than
    /// once per renderer.
    pub fn ensure(&mut self, size: u32) {
        let len = (size * size * 4) as usize;
        if self.size != size || self.px.len() != len {
            self.px.clear();
            self.px.resize(len, 0);
            self.size = size;
        }
    }
}

/// Where a tile lands on screen: the destination rect `blit` would fill, as
/// `(x0, y0, edge)` in px. `edge` is the scaled tile's edge length; the rect is
/// square because tiles are.
#[inline]
fn dest_rect(tile_size: u32, sx: f32, sy: f32, scale: f32) -> (i32, i32, i32) {
    let edge = (tile_size as f32 * scale).round().max(1.0) as i32;
    (
        (sx - edge as f32 * 0.5).floor() as i32,
        (sy - edge as f32 * 0.5).floor() as i32,
        edge,
    )
}

/// The sub-rect of a tile that [`blit`] will actually sample when the tile is
/// drawn at `(sx, sy)` scaled by `scale` — as `[tx0, ty0, tx1, ty1)` in TILE px.
/// Empty (`tx0 == tx1`) means the tile misses the viewport entirely.
///
/// This exists so a body renderer can shade only the part of its tile that will
/// be seen. Zoomed in far enough that a planet overflows the viewport, most of
/// its tile is off-screen — at a disc twice the viewport height, roughly 70% of
/// it — and shading that is pure waste: `blit` reads a tile pixel at most once
/// per destination pixel, so a tile pixel with no on-screen destination is never
/// read at all.
///
/// An empty result is also the exact visibility test, which is why callers can
/// use it in place of a hand-rolled "is this body off-screen" margin: it asks
/// the compositor where the tile really lands instead of guessing how far a
/// body's rings or corona reach.
///
/// Exact, not padded: `blit` reads tile pixel `map(dd)` for each destination
/// offset `dd` it visits, and `map` is non-decreasing in `dd`, so the tile
/// pixels it touches are exactly those between the two endpoints' images. The
/// two functions share the `map` expression below for that reason — keep them
/// in step, and keep `visible_rect_covers_every_tile_pixel_blit_reads` passing.
pub fn visible_tile_rect(tile_size: u32, w: u32, h: u32, sx: f32, sy: f32, scale: f32) -> [u32; 4] {
    let (x0, y0, edge) = dest_rect(tile_size, sx, sy, scale);
    // The on-screen slice of the destination rect, in destination-local px.
    let (ddx0, ddx1) = ((-x0).max(0), (w as i32 - x0).min(edge));
    let (ddy0, ddy1) = ((-y0).max(0), (h as i32 - y0).min(edge));
    if ddx1 <= ddx0 || ddy1 <= ddy0 {
        return [0, 0, 0, 0];
    }
    let inv = 1.0 / scale;
    // The half-open tile span the destination range [lo, hi) maps onto.
    let span = |lo: i32, hi: i32| {
        let map = |dd: i32| ((dd as f32 + 0.5) * inv) as u32;
        (map(lo), (map(hi - 1) + 1).min(tile_size))
    };
    let (tx0, tx1) = span(ddx0, ddx1);
    let (ty0, ty1) = span(ddy0, ddy1);
    if tx1 <= tx0 || ty1 <= ty0 {
        return [0, 0, 0, 0]; // the whole visible span is past the tile's edge
    }
    [tx0, ty0, tx1, ty1]
}

/// Alpha-blend a tile centred at screen `(sx, sy)` into the RGBA `out`,
/// nearest-neighbour scaled by `scale` (1.0 = 1:1). `scale > 1` blows each tile
/// pixel up into a crisp `scale`×`scale` block — this is how per-body pixelation
/// is applied: a body is rendered into a small tile, then upsized with hard
/// edges, so it turns blocky without changing its on-screen size.
///
/// Only the on-screen slice of the (possibly huge, when zoomed in) destination
/// rectangle is iterated — clamping the loop bounds instead of testing every
/// pixel keeps blit cost proportional to visible area, not tile size.
///
/// Within a row the destination is walked in RUNS of pixels that share one tile
/// pixel — the same trick `background-core::composite` uses for its cloud cells.
/// At the upscales a zoomed-in scene reaches, a run is tens of px long, so the
/// source fetch, the alpha test and the blend factors are computed once per run
/// instead of once per pixel, and a fully transparent run (a tile's corners are
/// all transparent) is skipped without touching the destination at all.
pub fn blit(out: &mut [u8], w: u32, h: u32, tile: &Tile, sx: f32, sy: f32, scale: f32) {
    let (x0, y0, dsize) = dest_rect(tile.size, sx, sy, scale);
    let inv = 1.0 / scale;
    let ddy0 = (-y0).max(0);
    let ddy1 = (h as i32 - y0).min(dsize);
    let ddx0 = (-x0).max(0);
    let ddx1 = (w as i32 - x0).min(dsize);
    for ddy in ddy0..ddy1 {
        let dy = y0 + ddy;
        let ty = ((ddy as f32 + 0.5) * inv) as u32;
        if ty >= tile.size {
            continue;
        }
        let row = (ty * tile.size) as usize;
        let mut ddx = ddx0;
        while ddx < ddx1 {
            let tx = ((ddx as f32 + 0.5) * inv) as u32;
            // The run ends where the next tile pixel starts: tx+1 <= (ddx+0.5)/scale.
            // `max(ddx + 1)` guarantees progress when scale <= 1 (a downscale
            // steps more than one tile px per destination px).
            let run_end = (((tx + 1) as f32 * scale - 0.5).ceil() as i32).clamp(ddx + 1, ddx1);
            if tx >= tile.size {
                ddx = run_end;
                continue;
            }
            let si = (row + tx as usize) * 4;
            let a = tile.px[si + 3] as u32;
            if a == 0 {
                ddx = run_end; // transparent: the whole run is a no-op
                continue;
            }
            let (sr, sg, sb) = (tile.px[si], tile.px[si + 1], tile.px[si + 2]);
            let di = ((dy as u32 * w + (x0 + ddx) as u32) * 4) as usize;
            let n = ((run_end - ddx) * 4) as usize;
            let dst = &mut out[di..di + n];
            if a == 255 {
                for p in dst.chunks_exact_mut(4) {
                    p[0] = sr;
                    p[1] = sg;
                    p[2] = sb;
                    p[3] = 255;
                }
            } else {
                // Premultiply the source side once; only the destination term
                // varies down the run.
                let ia = 255 - a;
                let (pr, pg, pb) = (sr as u32 * a, sg as u32 * a, sb as u32 * a);
                for p in dst.chunks_exact_mut(4) {
                    p[0] = ((pr + p[0] as u32 * ia) / 255) as u8;
                    p[1] = ((pg + p[1] as u32 * ia) / 255) as u8;
                    p[2] = ((pb + p[2] as u32 * ia) / 255) as u8;
                    p[3] = 255;
                }
            }
            ddx = run_end;
        }
    }
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// A tile with a bit of everything: opaque core, a translucent ring, fully
    /// transparent corners — so a blit exercises all three run kinds.
    fn tile(size: u32) -> Tile {
        let c = size as f32 / 2.0;
        let mut px = vec![0u8; (size * size * 4) as usize];
        for y in 0..size {
            for x in 0..size {
                let d = (((x as f32 + 0.5 - c).powi(2) + (y as f32 + 0.5 - c).powi(2)).sqrt()) / c;
                let i = ((y * size + x) * 4) as usize;
                let a = if d < 0.7 { 255 } else if d < 1.0 { 90 } else { 0 };
                px[i] = (x * 7 % 256) as u8;
                px[i + 1] = (y * 13 % 256) as u8;
                px[i + 2] = ((x + y) * 3 % 256) as u8;
                px[i + 3] = a;
            }
        }
        Tile { px, size }
    }

    /// The per-destination-pixel form `blit` replaced. The run-length walk must
    /// land on exactly these bytes — the compositor is the last thing every
    /// scene passes through, so a drift here shows up everywhere at once.
    fn blit_reference(out: &mut [u8], w: u32, h: u32, t: &Tile, sx: f32, sy: f32, scale: f32) {
        let dsize = (t.size as f32 * scale).round().max(1.0) as i32;
        let x0 = (sx - dsize as f32 * 0.5).floor() as i32;
        let y0 = (sy - dsize as f32 * 0.5).floor() as i32;
        let inv = 1.0 / scale;
        for ddy in (-y0).max(0)..(h as i32 - y0).min(dsize) {
            let dy = y0 + ddy;
            let ty = ((ddy as f32 + 0.5) * inv) as u32;
            if ty >= t.size {
                continue;
            }
            for ddx in (-x0).max(0)..(w as i32 - x0).min(dsize) {
                let dx = x0 + ddx;
                let tx = ((ddx as f32 + 0.5) * inv) as u32;
                if tx >= t.size {
                    continue;
                }
                let si = ((ty * t.size + tx) * 4) as usize;
                let a = t.px[si + 3] as u32;
                if a == 0 {
                    continue;
                }
                let di = ((dy as u32 * w + dx as u32) * 4) as usize;
                if a == 255 {
                    out[di] = t.px[si];
                    out[di + 1] = t.px[si + 1];
                    out[di + 2] = t.px[si + 2];
                    out[di + 3] = 255;
                } else {
                    let ia = 255 - a;
                    out[di] = ((t.px[si] as u32 * a + out[di] as u32 * ia) / 255) as u8;
                    out[di + 1] = ((t.px[si + 1] as u32 * a + out[di + 1] as u32 * ia) / 255) as u8;
                    out[di + 2] = ((t.px[si + 2] as u32 * a + out[di + 2] as u32 * ia) / 255) as u8;
                    out[di + 3] = 255;
                }
            }
        }
    }

    /// Placements worth covering: dead centre, hanging off each edge, bigger
    /// than the viewport (the zoomed-in case), fractional centres, and the
    /// downscale where a run is a single px.
    const PLACEMENTS: &[(f32, f32, f32)] = &[
        (60.0, 40.0, 1.0),
        (60.0, 40.0, 0.4),
        (60.0, 40.0, 3.0),
        (60.5, 40.25, 3.0),
        (60.0, 40.0, 11.0),
        (60.0, 40.0, 37.0),
        (-20.0, 40.0, 7.0),
        (140.0, 40.0, 7.0),
        (60.0, -30.0, 7.0),
        (60.0, 110.0, 7.0),
        (-3.5, -2.5, 5.5),
        (1000.0, 1000.0, 4.0),
    ];

    #[test]
    fn run_length_blit_matches_the_per_pixel_form() {
        let (w, h) = (121u32, 83u32);
        let t = tile(17);
        for &(sx, sy, scale) in PLACEMENTS {
            // Start from a non-uniform destination so the blend term is exercised.
            let base: Vec<u8> = (0..(w * h * 4)).map(|i| (i * 31 % 251) as u8).collect();
            let (mut got, mut want) = (base.clone(), base);
            blit(&mut got, w, h, &t, sx, sy, scale);
            blit_reference(&mut want, w, h, &t, sx, sy, scale);
            let bad = got.iter().zip(&want).position(|(a, b)| a != b);
            assert!(bad.is_none(), "blit diverged at byte {bad:?} for ({sx}, {sy}, x{scale})");
        }
    }

    /// `visible_tile_rect` is only safe to shade against if it is a SUPERSET of
    /// what `blit` reads. Under-reporting by one px would leave an unshaded
    /// seam along the edge of a zoomed-in planet, which is exactly the kind of
    /// bug that only shows up at the zoom levels nobody screenshots.
    /// The rect is exact rather than padded, so this is the only thing standing
    /// between a rounding change in either function and an unshaded seam down
    /// the edge of a zoomed-in body. Sweep far wider than `PLACEMENTS`: every
    /// scale from a heavy downscale to a 50x blow-up, at sub-pixel centres, on
    /// and off every edge.
    #[test]
    fn visible_rect_is_exact_across_a_wide_sweep() {
        let (w, h) = (121u32, 83u32);
        let mut checked = 0usize;
        for size in [6u32, 17, 64, 131] {
            for si in 0..80 {
                let scale = 0.05 + si as f32 * 0.65;
                for ci in 0..24 {
                    let (sx, sy) = (-40.0 + ci as f32 * 7.3, -30.0 + ci as f32 * 5.7);
                    let [tx0, ty0, tx1, ty1] = visible_tile_rect(size, w, h, sx, sy, scale);
                    let dsize = (size as f32 * scale).round().max(1.0) as i32;
                    let x0 = (sx - dsize as f32 * 0.5).floor() as i32;
                    let y0 = (sy - dsize as f32 * 0.5).floor() as i32;
                    let inv = 1.0 / scale;
                    for ddy in (-y0).max(0)..(h as i32 - y0).min(dsize) {
                        let ty = ((ddy as f32 + 0.5) * inv) as u32;
                        if ty >= size {
                            continue;
                        }
                        for ddx in (-x0).max(0)..(w as i32 - x0).min(dsize) {
                            let tx = ((ddx as f32 + 0.5) * inv) as u32;
                            if tx >= size {
                                continue;
                            }
                            assert!(
                                tx >= tx0 && tx < tx1 && ty >= ty0 && ty < ty1,
                                "size {size} at ({sx}, {sy}, x{scale}): blit reads tile \
                                 ({tx}, {ty}) outside [{tx0}, {ty0}, {tx1}, {ty1})"
                            );
                            checked += 1;
                        }
                    }
                }
            }
        }
        assert!(checked > 1_000_000, "only {checked} reads exercised");
    }

    #[test]
    fn visible_rect_covers_every_tile_pixel_blit_reads() {
        let (w, h) = (121u32, 83u32);
        for size in [6u32, 17, 64, 131] {
            for &(sx, sy, scale) in PLACEMENTS {
                let [tx0, ty0, tx1, ty1] = visible_tile_rect(size, w, h, sx, sy, scale);
                // Replay blit's own indexing and collect what it touches.
                let dsize = (size as f32 * scale).round().max(1.0) as i32;
                let x0 = (sx - dsize as f32 * 0.5).floor() as i32;
                let y0 = (sy - dsize as f32 * 0.5).floor() as i32;
                let inv = 1.0 / scale;
                let mut read = 0usize;
                for ddy in (-y0).max(0)..(h as i32 - y0).min(dsize) {
                    let ty = ((ddy as f32 + 0.5) * inv) as u32;
                    if ty >= size {
                        continue;
                    }
                    for ddx in (-x0).max(0)..(w as i32 - x0).min(dsize) {
                        let tx = ((ddx as f32 + 0.5) * inv) as u32;
                        if tx >= size {
                            continue;
                        }
                        assert!(
                            tx >= tx0 && tx < tx1 && ty >= ty0 && ty < ty1,
                            "size {size} at ({sx}, {sy}, x{scale}): blit reads tile ({tx}, {ty}) \
                             outside the reported rect [{tx0}, {ty0}, {tx1}, {ty1})"
                        );
                        read += 1;
                    }
                }
                // And the converse direction of usefulness: an empty rect must
                // mean blit really does read nothing.
                if tx1 == tx0 {
                    assert_eq!(read, 0, "reported empty but blit read {read} tile px");
                }
            }
        }
    }

    /// The whole point of the rect: when a body overflows the viewport it must
    /// report substantially less than the whole tile.
    #[test]
    fn a_tile_larger_than_the_viewport_reports_only_its_visible_part() {
        let (w, h) = (1680u32, 944u32);
        let size = 131;
        // scale 18 -> a 2358 px disc on a 944 px tall viewport
        let [tx0, ty0, tx1, ty1] = visible_tile_rect(size, w, h, w as f32 / 2.0, h as f32 / 2.0, 18.0);
        let frac = ((tx1 - tx0) * (ty1 - ty0)) as f32 / (size * size) as f32;
        assert!(frac < 0.45, "expected to skip most of the tile, kept {:.0}%", frac * 100.0);
        assert!(frac > 0.2, "kept suspiciously little of the tile: {:.0}%", frac * 100.0);
    }
}
