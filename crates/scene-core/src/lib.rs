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
pub struct Tile {
    pub px: Vec<u8>,
    pub size: u32,
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
pub fn blit(out: &mut [u8], w: u32, h: u32, tile: &Tile, sx: f32, sy: f32, scale: f32) {
    let dsize = (tile.size as f32 * scale).round().max(1.0) as i32;
    let x0 = (sx - dsize as f32 * 0.5).floor() as i32;
    let y0 = (sy - dsize as f32 * 0.5).floor() as i32;
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
        for ddx in ddx0..ddx1 {
            let dx = x0 + ddx;
            let tx = ((ddx as f32 + 0.5) * inv) as u32;
            if tx >= tile.size {
                continue;
            }
            let si = ((ty * tile.size + tx) * 4) as usize;
            let a = tile.px[si + 3] as u32;
            if a == 0 {
                continue;
            }
            let di = ((dy as u32 * w + dx as u32) * 4) as usize;
            if a == 255 {
                out[di] = tile.px[si];
                out[di + 1] = tile.px[si + 1];
                out[di + 2] = tile.px[si + 2];
                out[di + 3] = 255;
            } else {
                let ia = 255 - a;
                out[di] = ((tile.px[si] as u32 * a + out[di] as u32 * ia) / 255) as u8;
                out[di + 1] = ((tile.px[si + 1] as u32 * a + out[di + 1] as u32 * ia) / 255) as u8;
                out[di + 2] = ((tile.px[si + 2] as u32 * a + out[di + 2] as u32 * ia) / 255) as u8;
                out[di + 3] = 255;
            }
        }
    }
}
