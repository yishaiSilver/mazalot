//! background-core — the deep-space backdrop shared by every scene crate.
//!
//! Everything a scene paints *before* its bodies: a dithered navy ground, an
//! optional seeded [`Nebula`], and one or more parallax star layers. `solar`,
//! `moon`, `comet` and `asteroid` each carried a copy of this; the differences
//! between them were entirely constants — grid pitch, how many layers, how
//! bright, which star colours — so those now live in each crate's [`Backdrop`]
//! and [`Starfield`] rather than in a second implementation of the loop.
//!
//! Two passes, in this order:
//!   1. [`paint_backdrop`] — the ground, plus the nebula composited over it.
//!      This is the O(pixels) part, and the part worth caching (see
//!      [`BackdropCache`]): it depends only on the camera, never on time.
//!   2. [`paint_stars`] — additive 1-px points. Iterates the *visible grid
//!      cells* rather than the pixels, so its cost is O(stars on screen).
//!
//! Both are pure functions over an RGBA buffer. Nothing here knows about a
//! camera, a body, or a clock: a caller passes the accumulated screen-space pan
//! and whatever zoom-derived fades it wants.
//!
//! ## Parallax, and why it is screen-space
//!
//! A star layer is a fixed grid in SCREEN space, scrolled by `bgx`/`bgy` — the
//! camera's accumulated pan **in pixels** (Δcam·zoom summed over time) — at a
//! fraction of the foreground's rate. Anchoring in screen space (rather than
//! world space) means a layer does not respond to zoom at all, so a star can
//! never outrun the scene it sits behind, and the on-screen star count stays
//! constant at every zoom.

use std::cell::RefCell;

use dither_core::bayer;
use noise_core::{clamp01, fbm, hash3, mix, ramp, smoothstep, Rgb};

// ---------------------------------------------------------------------------
// Starfield
// ---------------------------------------------------------------------------

/// Star colours, as `(hash cutoff, colour)` stops for [`noise_core::ramp`] — a
/// hash in `[0, 1)` picks the first stop it falls under. Kept per-scene: a belt
/// and a solar system get to have different skies. End the table at a cutoff
/// above 1.0 so every hash lands somewhere.
pub type StarTints = &'static [(f32, Rgb)];

/// One parallax layer of a starfield.
#[derive(Clone, Copy)]
pub struct StarLayer {
    /// Fraction of the camera's screen-space pan this layer scrolls at. Smaller
    /// = further away. Keep every layer below 1.0 or stars overtake the scene.
    pub parallax: f32,
    /// Grid pitch in screen px. One star per lit cell, jittered within it.
    pub spacing: f32,
    /// Hash cutoff: a cell lights up when its hash exceeds this, so the lit
    /// fraction is `1 - threshold` (before [`Starfield::density`] scales it).
    pub threshold: f32,
    /// Brightness of the layer's brightest star.
    pub brightness: f32,
    /// The faintest star in the layer, as a fraction of `brightness`. Star
    /// brightness ramps from here to 1.0 with how far the cell cleared the
    /// threshold, which is what keeps a layer from reading as uniform dots.
    pub faint: f32,
    /// Decorrelates this layer's hash from its siblings.
    pub salt: i32,
}

/// A whole starfield: its layers, its palette, and the two live knobs.
pub struct Starfield<'a> {
    pub layers: &'a [StarLayer],
    pub tints: StarTints,
    /// Scales every layer's lit fraction. 1.0 = the layers as configured, 0 = an
    /// empty sky. Exactly 1.0 leaves each `threshold` bit-for-bit untouched.
    pub density: f32,
    /// Scales every layer's scroll rate — the user-facing "parallax" knob.
    /// 1.0 = as configured, 0 = stars pinned to the viewport.
    pub pan_scale: f32,
    /// Multiplier on the FIRST layer only, for fading the most distant stars out
    /// as the camera zooms in on a body. 1.0 = no fade; at or below 0.02 the
    /// layer is skipped entirely.
    pub far_fade: f32,
}

impl<'a> Starfield<'a> {
    /// A field with both knobs neutral and no far-layer fade — the common case
    /// for a scene that doesn't expose them.
    pub fn new(layers: &'a [StarLayer], tints: StarTints) -> Starfield<'a> {
        Starfield { layers, tints, density: 1.0, pan_scale: 1.0, far_fade: 1.0 }
    }
}

/// Plot the starfield additively over whatever is already in `out`.
///
/// `bgx`/`bgy` are the camera's accumulated pan in screen px. `hash` maps a grid
/// cell and a layer's salt to `[0, 1)` — each scene mixes its own seed in there
/// however it likes (or ignores the seed, for a sky shared by every scene).
///
/// Only the visible cell range is iterated, so cost tracks stars on screen, not
/// viewport area.
pub fn paint_stars<F>(out: &mut [u8], w: u32, h: u32, sky: &Starfield, bgx: f32, bgy: f32, hash: F)
where
    F: Fn(i32, i32, i32) -> f32,
{
    let d = sky.density.max(0.0);
    let (wi, hi) = (w as i32, h as i32);

    for (i, layer) in sky.layers.iter().enumerate() {
        // The far layer fades (and is skipped) when zoomed in on a body.
        let amt = if i == 0 { sky.far_fade } else { 1.0 };
        if amt <= 0.02 {
            continue;
        }
        let thr = 1.0 - (1.0 - layer.threshold) * d;
        if thr >= 0.9999 {
            continue; // density ~0 -> no stars in this layer
        }
        let inv = 1.0 / layer.spacing;
        let (ox, oy) = (bgx * layer.parallax * sky.pan_scale, bgy * layer.parallax * sky.pan_scale);
        let (c0x, c1x) = ((ox * inv).floor() as i32 - 1, ((ox + w as f32) * inv).floor() as i32 + 1);
        let (c0y, c1y) = ((oy * inv).floor() as i32 - 1, ((oy + h as f32) * inv).floor() as i32 + 1);
        for cy in c0y..=c1y {
            for cx in c0x..=c1x {
                let hh = hash(cx, cy, layer.salt);
                if hh <= thr {
                    continue;
                }
                let jx = (hh * 137.0).fract(); // jitter across the cell, [0,1)
                let jy = (hh * 71.3 + 0.37).fract();
                let px = ((cx as f32 + jx) * layer.spacing - ox).floor() as i32;
                let py = ((cy as f32 + jy) * layer.spacing - oy).floor() as i32;
                if px < 0 || py < 0 || px >= wi || py >= hi {
                    continue;
                }
                // How far this cell cleared the threshold sets its brightness.
                let t = (hh - thr) / (1.0 - thr);
                let s = layer.brightness * (layer.faint + (1.0 - layer.faint) * t) * amt;
                let col = ramp(sky.tints, (hh * 313.0).fract());
                let idx = ((py as u32 * w + px as u32) * 4) as usize;
                out[idx] = (clamp01(out[idx] as f32 / 255.0 + s * col[0]) * 255.0) as u8;
                out[idx + 1] = (clamp01(out[idx + 1] as f32 / 255.0 + s * col[1]) * 255.0) as u8;
                out[idx + 2] = (clamp01(out[idx + 2] as f32 / 255.0 + s * col[2]) * 255.0) as u8;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Ground + nebula
// ---------------------------------------------------------------------------

/// A faint interstellar cloud behind everything: low-frequency fBm sampled once
/// per `cell`×`cell` block (so it reads as pixel-art rather than a smooth
/// gradient) and tinted by two colours drawn from `tints` by seed.
#[derive(Clone, Copy)]
pub struct Nebula {
    /// Palette to draw this scene's two tints from. Keep them low-saturation —
    /// the nebula sits under everything and should never compete with a body.
    pub tints: &'static [Rgb],
    /// Bake resolution: one fBm sample per `cell`×`cell` px block.
    pub cell: u32,
    /// The scroll offset is snapped to this many px, so a small pan (and every
    /// zoom) reuses the previous bake instead of re-running the per-cell fBm.
    pub quant: f32,
    /// Fraction of the camera's pan the clouds drift at — the slowest layer in
    /// the scene by some margin.
    pub scroll: f32,
    /// Overall opacity of the baked field.
    pub strength: f32,
    /// Ordered-dither amplitude applied with the nebula, turning its gradient
    /// into pixel-art stipple.
    pub dither: f32,
}

/// The ground a scene is painted on, and what floats in it.
pub struct Backdrop {
    /// Base fill — the colour of empty space in this scene.
    pub base: Rgb,
    /// Ordered-dither amplitude on the base fill, so a flat field still reads as
    /// pixel-art rather than a dead block. 0 = a perfectly flat fill (and then
    /// `base` round-trips exactly, so `[8.0/255.0, ..]` yields byte 8).
    pub dither: f32,
    /// `None` for a plain field of stars.
    pub nebula: Option<Nebula>,
}

/// Two nested caches over the backdrop, both exploiting that it "barely
/// changes": it depends on the camera and the view knobs, never on animation
/// time.
///
///   • `neb_field` — the low-res per-cell fBm, the most expensive single pass.
///     Keyed on the scroll offset ONLY, so a pure zoom never re-bakes it.
///   • `layer` — the full-res composite of ground + nebula. Keyed on scroll AND
///     the zoom fade. This is the big one: on a drag it is reused as a memcpy,
///     collapsing the O(pixels) fill to a copy so only the stars redraw.
///
/// Stars are never cached here — they scroll every frame, they *are* the
/// parallax, and they're cheap.
#[derive(Default)]
pub struct BackdropCache {
    /// `[nw, nh, qx, qy]` the `neb_field` is valid for (zoom deliberately excluded).
    neb_key: Option<[i32; 4]>,
    /// Low-res RGB nebula at full strength (density premultiplied, no zoom fade
    /// — that is applied per-pixel at composite time) — `nw * nh` entries.
    neb_field: Vec<[f32; 3]>,
    /// `[w, h, qx, qy, quantized zoom fade]` the `layer` is valid for.
    layer_key: Option<[i32; 5]>,
    /// Full-res RGBA ground + nebula (no stars) — `w * h * 4` bytes.
    layer: Vec<u8>,
}

/// Re-bake the cached nebula field if `[nw, nh, qx, qy]` moved. Stores it at
/// FULL strength — the zoom fade is applied per-pixel at composite, which is
/// exactly why the key can exclude zoom.
fn ensure_nebula(neb: &Nebula, cache: &RefCell<BackdropCache>, si: i32, nw: u32, nh: u32, qx: i32, qy: i32) {
    let key = [nw as i32, nh as i32, qx, qy];
    if let Ok(c) = cache.try_borrow() {
        if c.neb_key == Some(key) && c.neb_field.len() == (nw * nh) as usize {
            return; // hit — the clouds barely moved (or only the zoom changed)
        }
    }
    let mut c = cache.borrow_mut();
    c.neb_field.clear();
    c.neb_field.resize((nw * nh) as usize, [0.0; 3]);
    bake_nebula(neb, &mut c.neb_field, si, nw, nh, qx, qy);
    c.neb_key = Some(key);
}

/// The per-cell fBm itself: patchy density thresholded out of one noise field,
/// tinted by mixing the scene's two seeded colours with a second.
fn bake_nebula(neb: &Nebula, field: &mut [[f32; 3]], si: i32, nw: u32, nh: u32, qx: i32, qy: i32) {
    let n = neb.tints.len();
    let ta = neb.tints[(hash3(si, 1, 9) * n as f32) as usize % n];
    let tb = neb.tints[(hash3(si, 2, 9) * n as f32) as usize % n];
    let (nox, noy) = (qx as f32 * neb.quant, qy as f32 * neb.quant);
    let f = 1.0 / 240.0;
    for cy in 0..nh {
        for cx in 0..nw {
            let gx = ((cx * neb.cell) as f32 + nox) * f;
            let gy = ((cy * neb.cell) as f32 + noy) * f;
            let dens = smoothstep(0.50, 0.74, fbm(gx, gy, 4.2, 3)); // patchy -> not crowded
            if dens > 0.0 {
                let n2 = fbm(gx * 1.8 + 40.0, gy * 1.8 + 7.0, 1.5, 2);
                let col = mix(ta, tb, clamp01((n2 - 0.35) * 2.2));
                let k = dens * neb.strength; // zoom fade applied later at composite
                field[(cy * nw + cx) as usize] = [col[0] * k, col[1] * k, col[2] * k];
            }
        }
    }
}

/// Fill `out` with the ground, compositing `field` (may be empty) over it.
fn composite(out: &mut [u8], w: u32, h: u32, cfg: &Backdrop, field: &[[f32; 3]], nw: u32, neb_amt: f32) {
    let has_neb = !field.is_empty();
    let (cell, neb_dither) = match &cfg.nebula {
        Some(n) => (n.cell, n.dither),
        None => (1, 0.0),
    };
    for iy in 0..h {
        let nrow = (iy / cell) * nw;
        for ix in 0..w {
            let (mut r, mut g, mut b) = (cfg.base[0], cfg.base[1], cfg.base[2]);
            if cfg.dither > 0.0 {
                let d = bayer(ix, iy) * cfg.dither;
                r += d;
                g += d;
                b += d;
            }
            if has_neb {
                let c = field[(nrow + ix / cell) as usize];
                let d = bayer(ix, iy) * neb_dither; // dither -> pixel-art gradient
                r += (c[0] * neb_amt + d).max(0.0);
                g += (c[1] * neb_amt + d).max(0.0);
                b += (c[2] * neb_amt + d).max(0.0);
            }
            let idx = ((iy * w + ix) * 4) as usize;
            out[idx] = (clamp01(r) * 255.0) as u8;
            out[idx + 1] = (clamp01(g) * 255.0) as u8;
            out[idx + 2] = (clamp01(b) * 255.0) as u8;
            out[idx + 3] = 255;
        }
    }
}

/// Paint the ground and its nebula into `out` (RGBA, `w*h*4` bytes).
///
/// `bgx`/`bgy` are the camera's accumulated screen-space pan and `pan_scale` the
/// live parallax knob; `neb_amt` fades the clouds out as the camera zooms in
/// (1.0 = full, at or below 0.02 the nebula is skipped and not even baked).
///
/// Pass a `cache` to get the memcpy fast path on a still or slowly-dragging
/// camera. Without one the result is identical, just recomputed every call.
/// `#[inline]` matters here: callers pass a `&'static Backdrop` const, so inlining
/// lets the optimizer fold `nebula: None` away and strip the whole bake path from
/// a scene that doesn't use one.
#[inline]
#[allow(clippy::too_many_arguments)]
pub fn paint_backdrop(
    out: &mut [u8],
    w: u32,
    h: u32,
    cfg: &Backdrop,
    seed: u32,
    bgx: f32,
    bgy: f32,
    pan_scale: f32,
    neb_amt: f32,
    cache: Option<&RefCell<BackdropCache>>,
) {
    let len = (w * h * 4) as usize;
    let si = seed as i32;

    // Zoomed in far enough and the clouds are gone; then the ground is a plain
    // fill that depends on nothing but its own size, so the scroll offset drops
    // out of the cache key and a pan stops invalidating it.
    let show = cfg.nebula.is_some() && neb_amt > 0.02;
    let (nw, nh, qx, qy) = match cfg.nebula {
        Some(neb) if show => {
            let np = neb.scroll * pan_scale; // clouds drift slowest on screen
            (
                (w + neb.cell - 1) / neb.cell,
                (h + neb.cell - 1) / neb.cell,
                ((bgx * np + hash3(si, 5, 2) * 500.0) / neb.quant).round() as i32,
                ((bgy * np + hash3(si, 6, 2) * 500.0) / neb.quant).round() as i32,
            )
        }
        _ => (1, 1, 0, 0),
    };

    let Some(cache) = cache else {
        // Uncached: bake into a scratch field (if any) and composite once.
        let mut field = Vec::new();
        if let (true, Some(neb)) = (show, cfg.nebula) {
            field.resize((nw * nh) as usize, [0.0f32; 3]);
            bake_nebula(&neb, &mut field, si, nw, nh, qx, qy);
        }
        composite(out, w, h, cfg, &field, nw, neb_amt);
        return;
    };

    // Keyed on the scroll offset AND the quantized zoom fade: on a drag between
    // offset ticks this whole pass is a memcpy, and only a large pan or a zoom
    // step rebuilds it.
    let layer_key = [w as i32, h as i32, qx, qy, if show { (neb_amt * 40.0).round() as i32 } else { -1 }];
    let hit = {
        let c = cache.borrow();
        c.layer_key == Some(layer_key) && c.layer.len() == len
    };
    if hit {
        out[..len].copy_from_slice(&cache.borrow().layer);
        return;
    }

    // Miss. The per-cell fBm bake is itself cached and survives zoom, so a
    // zoom-only rebuild skips it and just re-composites.
    if let (true, Some(neb)) = (show, cfg.nebula) {
        ensure_nebula(&neb, cache, si, nw, nh, qx, qy);
    }
    {
        let c = cache.borrow();
        let field: &[[f32; 3]] = if show { &c.neb_field } else { &[] };
        composite(out, w, h, cfg, field, nw, neb_amt);
    }
    let mut c = cache.borrow_mut();
    c.layer.clear();
    c.layer.extend_from_slice(&out[..len]);
    c.layer_key = Some(layer_key);
}
