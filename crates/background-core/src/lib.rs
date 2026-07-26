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
//! ## The backdrop is a scrolling sprite
//!
//! [`paint_backdrop`] is the expensive half, and it is not re-evaluated per
//! frame. It depends on where the camera is, never on what time it is, so a
//! frame is mostly the previous frame *moved* — and [`BackdropCache`] treats it
//! that way, at two levels (see there for the split):
//!
//!   • **still, or panned less than a `quant`** — a memcpy of the last frame;
//!   • **zoomed** — a re-composite, but nothing re-baked: the fade is applied
//!     when the cloud sprite is read, so zoom never invalidates it;
//!   • **panned** — a memmove of both sprites, then a repaint of only the strip
//!     that scrolled into view, a percent or two of the screen;
//!   • **resized, or jumped far enough that the views don't overlap** — the full
//!     rebuild, which is what a pan used to cost every frame.
//!
//! The whole scheme rests on the backdrop being a function of where the clouds
//! are, so that moving them moves it. That is why the nebula's dither travels
//! with the clouds rather than being pinned to the screen, and why a scene that
//! sets [`Backdrop::dither`] under a nebula opts out of the scrolled path.
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
    /// The scroll offset is snapped to this many px, so a sub-tick pan (and
    /// every zoom) reuses the previous frame outright. Use a whole number of px:
    /// the offset indexes a baked sprite, so a fractional `quant` would round
    /// and leave the clouds jittering by a px.
    pub quant: f32,
    /// Fraction of the camera's pan the clouds drift at — the slowest layer in
    /// the scene by some margin.
    pub scroll: f32,
    /// Overall opacity of the baked field.
    pub strength: f32,
    /// Ordered-dither amplitude applied with the nebula, turning its gradient
    /// into pixel-art stipple. Anchored to the CLOUDS, not the screen, so the
    /// stipple travels with them instead of the clouds crawling through a fixed
    /// screen pattern — which is also what lets the whole layer be scrolled.
    pub dither: f32,
}

/// The ground a scene is painted on, and what floats in it.
pub struct Backdrop {
    /// Base fill — the colour of empty space in this scene.
    pub base: Rgb,
    /// Ordered-dither amplitude on the base fill, so a flat field still reads as
    /// pixel-art rather than a dead block. 0 = a perfectly flat fill (and then
    /// `base` round-trips exactly, so `[8.0/255.0, ..]` yields byte 8).
    ///
    /// Unlike the nebula's, this dither is pinned to the SCREEN — the ground
    /// does not drift, so there is nothing for it to travel with. A scene that
    /// sets both this and a `nebula` therefore gives up the scrolled-layer fast
    /// path below, since its frame is then no longer a pure function of where
    /// the clouds are. None currently does.
    pub dither: f32,
    /// `None` for a plain field of stars.
    pub nebula: Option<Nebula>,
}

/// Two nested sprite caches over the backdrop, both exploiting the same thing:
/// the backdrop depends on where the camera is, never on what time it is, so a
/// frame is mostly the previous frame *moved*.
///
///   • `neb_field` — the low-res per-cell fBm. Indexed by absolute world cell,
///     so a pan slides it and re-bakes only the strip that scrolled in.
///   • `layer` — the full-res RGBA composite of ground + nebula, and the more
///     expensive of the two by roughly 4:1. Scrolled the same way: on a drag it
///     is memmoved and only the newly-exposed strip is re-composited, so a pan
///     costs a sliver of a screenful instead of the whole thing.
///
/// Both survive zoom untouched — the zoom fade is applied when the field is
/// read, so it is not baked in — and both fall back to a full rebuild on a
/// resize or a jump big enough that the old and new views don't overlap.
///
/// Stars are never cached here — they scroll every frame, they *are* the
/// parallax, and they're cheap.
#[derive(Default)]
pub struct BackdropCache {
    /// `[nw, nh]` the `neb_field` is sized for.
    neb_dims: Option<[i32; 2]>,
    /// World cell coordinate of `neb_field[0]` — where the sprite is scrolled to.
    neb_org: Option<[i32; 2]>,
    /// Low-res RGB nebula at full strength (density premultiplied, no zoom fade
    /// — that is applied per-pixel at composite time) — `nw * nh` entries.
    neb_field: Vec<[f32; 3]>,
    /// `[w, h, quantized zoom fade]` the `layer` is sized and shaded for.
    layer_key: Option<[i32; 3]>,
    /// Scroll offset in px the `layer` is composited at.
    layer_org: Option<[i32; 2]>,
    /// Full-res RGBA ground + nebula (no stars) — `w * h * 4` bytes.
    layer: Vec<u8>,
}

/// Slide a 2D buffer in place by `(dx, dy)` whole units — `unit` elements each,
/// `nw` × `nh` of them — so that afterwards unit `(x, y)` holds what unit
/// `(x + dx, y + dy)` did.
///
/// The units with no source, i.e. the strips that just scrolled into view, are
/// left holding stale data for the caller to repaint. Shared by both caches:
/// the cloud field scrolls in cells of one `[f32; 3]`, the composited layer in
/// pixels of four bytes.
fn scroll<T: Copy>(buf: &mut [T], nw: u32, nh: u32, unit: usize, dx: i32, dy: i32) {
    let (nwi, nhi) = (nw as i32, nh as i32);
    // Destination columns that have a source: x + dx must land inside the buffer.
    let (x0, x1) = ((-dx).max(0), (nwi - dx).min(nwi));
    if x1 <= x0 {
        return;
    }
    let (run, stride) = ((x1 - x0) as usize * unit, nw as usize * unit);
    for k in 0..nhi {
        // Walk rows away from the source: reading ahead of the write when the
        // buffer slides up, behind it when it slides down. Either way a row is
        // copied before anything can clobber it.
        let y = if dy >= 0 { k } else { nhi - 1 - k };
        let sy = y + dy;
        if sy < 0 || sy >= nhi {
            continue; // scrolled in from off-buffer; the caller repaints it
        }
        let dst = y as usize * stride + x0 as usize * unit;
        let src = sy as usize * stride + (x0 + dx) as usize * unit;
        buf.copy_within(src..src + run, dst); // memmove: handles dy == 0 overlap
    }
}

/// The two strips a scroll of `(dx, dy)` exposes in an `nw` × `nh` buffer, as
/// half-open rects `[x0, y0, x1, y1)`. Either may be empty (`x0 == x1`).
///
/// When both axes moved the corner falls in both rects and is painted twice;
/// that is a few dozen units, not worth the bookkeeping to avoid.
fn exposed(nw: u32, nh: u32, dx: i32, dy: i32) -> [[u32; 4]; 2] {
    let col = match dx {
        d if d > 0 => [nw - d as u32, 0, nw, nh],
        d if d < 0 => [0, 0, -d as u32, nh],
        _ => [0, 0, 0, 0],
    };
    let row = match dy {
        d if d > 0 => [0, nh - d as u32, nw, nh],
        d if d < 0 => [0, 0, nw, -d as u32],
        _ => [0, 0, 0, 0],
    };
    [col, row]
}

/// Bring the cached cloud sprite to world cell origin `org`, baking as little as
/// possible. Stored at FULL strength — the zoom fade is applied per-pixel at
/// composite, which is why zoom never invalidates it.
///
/// Three outcomes, cheapest first: already there (nothing), scrolled by a few
/// cells (memmove plus the exposed strips), or a jump/resize (full bake).
fn ensure_nebula(neb: &Nebula, cache: &RefCell<BackdropCache>, si: i32, nw: u32, nh: u32, org: [i32; 2]) {
    let n = (nw * nh) as usize;
    let dims = Some([nw as i32, nh as i32]);
    if let Ok(c) = cache.try_borrow() {
        if c.neb_dims == dims && c.neb_org == Some(org) && c.neb_field.len() == n {
            return; // hit — the clouds haven't crossed a cell (or only zoom moved)
        }
    }
    let mut c = cache.borrow_mut();
    // A resize invalidates the sprite outright; otherwise it can be slid.
    let prev = if c.neb_dims == dims && c.neb_field.len() == n { c.neb_org } else { None };
    match prev {
        // Scrolled, and the two views still overlap: this is the whole point — a
        // drag pays for the sliver of new sky, not the screenful it already has.
        Some([px, py]) if (org[0] - px).abs() < nw as i32 && (org[1] - py).abs() < nh as i32 => {
            let (dx, dy) = (org[0] - px, org[1] - py);
            scroll(&mut c.neb_field, nw, nh, 1, dx, dy);
            for r in exposed(nw, nh, dx, dy) {
                bake_cells(neb, &mut c.neb_field, si, nw, org, r);
            }
        }
        _ => {
            c.neb_field.clear();
            c.neb_field.resize(n, [0.0; 3]);
            bake_cells(neb, &mut c.neb_field, si, nw, org, [0, 0, nw, nh]);
        }
    }
    c.neb_dims = dims;
    c.neb_org = Some(org);
}

/// The per-cell fBm itself: patchy density thresholded out of one noise field,
/// tinted by mixing the scene's two seeded colours with a second.
///
/// Bakes the half-open cell rect `[x0, y0, x1, y1)` of a field whose cell
/// `(x, y)` is the cloud at world px `((org[0] + x) * cell, (org[1] + y) * cell)`
/// — an absolute lattice, which is what lets the field be scrolled rather than
/// rebuilt. Every visited slot is written, empty ones included: a scrolled field
/// bakes over whatever the previous position left behind.
fn bake_cells(neb: &Nebula, field: &mut [[f32; 3]], si: i32, nw: u32, org: [i32; 2], rect: [u32; 4]) {
    let n = neb.tints.len();
    let ta = neb.tints[(hash3(si, 1, 9) * n as f32) as usize % n];
    let tb = neb.tints[(hash3(si, 2, 9) * n as f32) as usize % n];
    // The noise is 3D, but a nebula only ever samples one x/y plane of it — so
    // WHERE that plane sits is what makes one seed's clouds a different shape
    // rather than the same clouds slid sideways. Held constant across a bake so
    // a scrolled-in strip lands on the same plane as the field it joins.
    // 64 spans many lattice cells at this frequency, so two seeds land on
    // uncorrelated slices rather than neighbouring ones.
    let za = 4.2 + hash3(si, 7, 3) * 64.0;
    let zb = 1.5 + hash3(si, 8, 3) * 64.0;
    let cell = neb.cell as i32;
    let f = 1.0 / 240.0;
    let [x0, y0, x1, y1] = rect;
    for cy in y0..y1 {
        let gy = ((org[1] + cy as i32) * cell) as f32 * f;
        for cx in x0..x1 {
            let gx = ((org[0] + cx as i32) * cell) as f32 * f;
            let dens = smoothstep(0.50, 0.74, fbm(gx, gy, za, 3)); // patchy -> not crowded
            field[(cy * nw + cx) as usize] = if dens > 0.0 {
                let n2 = fbm(gx * 1.8 + 40.0, gy * 1.8 + 7.0, zb, 2);
                let col = mix(ta, tb, clamp01((n2 - 0.35) * 2.2));
                let k = dens * neb.strength; // zoom fade applied later at composite
                [col[0] * k, col[1] * k, col[2] * k]
            } else {
                [0.0; 3]
            };
        }
    }
}

/// Paint the ground and its clouds into the px rect `[x0, y0, x1, y1)` of an
/// RGBA buffer `w` px wide.
///
/// `sub` is the scroll's leftover sub-cell offset, `[0, cell)` px on each axis:
/// the cloud sprite is only positioned to whole cells, so the last few px of the
/// drift are paid here, by reading it shifted. That is why the field is baked
/// one cell wider and taller than the viewport needs. `phase` is the same idea
/// for the dither — the offset that keeps its pattern anchored to the clouds.
///
/// Two loops rather than one with an `if`, because `cell` is a runtime value: a
/// per-pixel `ix / cell` is a real integer division. The nebula path instead
/// walks each row in the runs that share a field cell, so that division and the
/// field load happen once per `cell` px, with the zoom fade folded in per run.
#[allow(clippy::too_many_arguments)]
fn composite(out: &mut [u8], w: u32, cfg: &Backdrop, field: &[[f32; 3]], nw: u32, sub: [u32; 2], phase: [u32; 2], neb_amt: f32, rect: [u32; 4]) {
    let (base, bd) = (cfg.base, cfg.dither);
    let [x0, y0, x1, y1] = rect;

    // No clouds to draw — either this scene has none, or they have faded out.
    // Then there is nothing to index and a row is a straight fill.
    let Some(neb) = cfg.nebula.filter(|_| !field.is_empty()) else {
        for iy in y0..y1 {
            for ix in x0..x1 {
                let d = if bd > 0.0 { bayer(ix, iy) * bd } else { 0.0 };
                let idx = ((iy * w + ix) * 4) as usize;
                out[idx] = (clamp01(base[0] + d) * 255.0) as u8;
                out[idx + 1] = (clamp01(base[1] + d) * 255.0) as u8;
                out[idx + 2] = (clamp01(base[2] + d) * 255.0) as u8;
                out[idx + 3] = 255;
            }
        }
        return;
    };

    let cell = neb.cell;
    for iy in y0..y1 {
        let nrow = ((iy + sub[1]) / cell) * nw;
        let by = iy + phase[1];
        let mut ix = x0;
        while ix < x1 {
            // The run of px sharing this field cell: it ends where the next cell
            // starts, `(fx + 1) * cell` in field space, minus the sub-cell shift.
            let fx = (ix + sub[0]) / cell;
            let end = ((fx + 1) * cell - sub[0]).min(x1);
            let c = field[(nrow + fx) as usize];
            let (cr, cg, cb) = (c[0] * neb_amt, c[1] * neb_amt, c[2] * neb_amt);
            for x in ix..end {
                let bx = bayer(x + phase[0], by);
                let d = bx * neb.dither; // dither -> pixel-art stipple
                let g = if bd > 0.0 { bayer(x, iy) * bd } else { 0.0 };
                let idx = ((iy * w + x) * 4) as usize;
                out[idx] = (clamp01(base[0] + g + (cr + d).max(0.0)) * 255.0) as u8;
                out[idx + 1] = (clamp01(base[1] + g + (cg + d).max(0.0)) * 255.0) as u8;
                out[idx + 2] = (clamp01(base[2] + g + (cb + d).max(0.0)) * 255.0) as u8;
                out[idx + 3] = 255;
            }
            ix = end;
        }
    }
}

/// Paint the ground and its nebula into `out` (RGBA, `w*h*4` bytes).
///
/// `bgx`/`bgy` are the camera's accumulated screen-space pan and `pan_scale` the
/// live parallax knob; `neb_amt` fades the clouds out as the camera zooms in
/// (1.0 = full, at or below 0.02 the nebula is skipped and not even baked).
///
/// Pass a `cache` to get the sprite fast paths: a memcpy on a still camera, and
/// on a drag a memmove plus a re-composite of only the strip that scrolled in.
/// Without one the result is identical, just recomputed every call.
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
    let (nw, nh, org, sub, phase, sx, sy) = match cfg.nebula {
        Some(neb) if show => {
            let np = neb.scroll * pan_scale; // clouds drift slowest on screen
            let cell = neb.cell as i32;
            // Where the clouds have drifted to, in whole px, snapped to `quant`.
            let q = |v: f32, salt: i32| -> i32 {
                (((v * np + hash3(si, salt, 2) * 500.0) / neb.quant).round() * neb.quant).round() as i32
            };
            let (sx, sy) = (q(bgx, 5), q(bgy, 6));
            // Split the drift. The sprite is baked on the absolute cell lattice,
            // so it scrolls by whole cells; the sub-cell remainder is applied
            // when the field is READ, which needs one spare column and row.
            (
                w.div_ceil(neb.cell) + 1,
                h.div_ceil(neb.cell) + 1,
                [sx.div_euclid(cell), sy.div_euclid(cell)],
                [sx.rem_euclid(cell) as u32, sy.rem_euclid(cell) as u32],
                // The dither rides along with the clouds, mod its 8-px period.
                [sx.rem_euclid(8) as u32, sy.rem_euclid(8) as u32],
                sx,
                sy,
            )
        }
        _ => (1, 1, [0, 0], [0, 0], [0, 0], 0, 0),
    };

    let Some(cache) = cache else {
        // Uncached: bake into a scratch field (if any) and composite once.
        let mut field = Vec::new();
        if let (true, Some(neb)) = (show, cfg.nebula) {
            field.resize((nw * nh) as usize, [0.0f32; 3]);
            bake_cells(&neb, &mut field, si, nw, org, [0, 0, nw, nh]);
        }
        composite(out, w, cfg, &field, nw, sub, phase, neb_amt, [0, 0, w, h]);
        return;
    };

    // The cloud sprite survives both zoom and pan, so this is usually free: a
    // zoom-only frame bakes nothing at all, and a drag bakes one strip of cells.
    if let (true, Some(neb)) = (show, cfg.nebula) {
        ensure_nebula(&neb, cache, si, nw, nh, org);
    }

    let mut borrow = cache.borrow_mut();
    let c = &mut *borrow;
    // Sized and shaded for this frame? The fade is baked into the layer (unlike
    // the field), so it belongs in the key; the scroll offset does not, because
    // a scroll is repaired rather than invalidating.
    let key = [w as i32, h as i32, if show { (neb_amt * 40.0).round() as i32 } else { -1 }];
    let sized = c.layer_key == Some(key) && c.layer.len() == len;
    if sized && c.layer_org == Some([sx, sy]) {
        out[..len].copy_from_slice(&c.layer); // still, or panned less than a `quant`
        return;
    }
    if !sized {
        c.layer.clear();
        c.layer.resize(len, 0);
    }
    let field: &[[f32; 3]] = if show { &c.neb_field } else { &[] };

    // A scrolled layer is only reusable if every pixel of it is a function of
    // where the CLOUDS are — which is why the nebula's dither is anchored to
    // them. A screen-pinned ground dither underneath would not survive the
    // memmove, so such a scene re-composites in full.
    let slide = match (sized, c.layer_org) {
        (true, Some([px, py])) if cfg.dither == 0.0 => {
            let (dx, dy) = (sx - px, sy - py);
            (dx.abs() < w as i32 && dy.abs() < h as i32).then_some((dx, dy))
        }
        _ => None,
    };
    match slide {
        Some((dx, dy)) => {
            scroll(&mut c.layer, w, h, 4, dx, dy);
            for r in exposed(w, h, dx, dy) {
                composite(&mut c.layer, w, cfg, field, nw, sub, phase, neb_amt, r);
            }
        }
        None => composite(&mut c.layer, w, cfg, field, nw, sub, phase, neb_amt, [0, 0, w, h]),
    }
    c.layer_key = Some(key);
    c.layer_org = Some([sx, sy]);
    out[..len].copy_from_slice(&c.layer);
}

// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const TINTS: &[Rgb] = &[[0.30, 0.22, 0.48], [0.18, 0.30, 0.46], [0.42, 0.24, 0.30]];

    /// solar's backdrop, near enough — the only one in the workspace with clouds.
    const CLOUDY: Backdrop = Backdrop {
        base: [0.031, 0.027, 0.068],
        dither: 0.0,
        nebula: Some(Nebula { tints: TINTS, cell: 8, quant: 2.0, scroll: 0.09, strength: 0.34, dither: 0.015 }),
    };

    /// A scene with a ground dither and no clouds — the other four crates.
    const PLAIN: Backdrop = Backdrop { base: [0.02, 0.02, 0.05], dither: 0.02, nebula: None };

    /// The uncached path is the reference: it bakes and composites the whole
    /// frame from scratch. Every fast path — memcpy, scroll-and-patch, re-bake —
    /// must land on exactly the same bytes, or a drag leaves streaks of stale
    /// sky that no still frame would ever show.
    fn agrees_with_uncached(cfg: &Backdrop, w: u32, h: u32, steps: &[(f32, f32, f32)]) {
        let len = (w * h * 4) as usize;
        let cache = RefCell::new(BackdropCache::default());
        let (mut cached, mut fresh) = (vec![0u8; len], vec![0u8; len]);
        for &(bgx, bgy, amt) in steps {
            paint_backdrop(&mut cached, w, h, cfg, 7, bgx, bgy, 1.0, amt, Some(&cache));
            paint_backdrop(&mut fresh, w, h, cfg, 7, bgx, bgy, 1.0, amt, None);
            let bad = cached.iter().zip(&fresh).position(|(a, b)| a != b);
            assert!(bad.is_none(), "cached backdrop diverged at byte {:?}, pan ({bgx}, {bgy}), fade {amt}", bad);
        }
    }

    /// The clouds drift at `scroll` (0.09) of the pan, so these are chosen for
    /// what they do to the SPRITE: a sub-quant nudge, a sub-cell tick, single-
    /// and multi-cell slides, both directions at once, a reversal, a fade-only
    /// change, and a jump past the field with no overlap left to reuse.
    const PANS: &[(f32, f32, f32)] = &[
        (0.0, 0.0, 1.0),      // first frame: full bake
        (2.0, 0.0, 1.0),      // < 1 quant of drift: pure memcpy
        (24.0, 0.0, 1.0),     // ~2 px: sub-cell, layer scrolls, field does not
        (120.0, 0.0, 1.0),    // ~11 px: over a cell boundary
        (120.0, 95.0, 1.0),   // vertical only
        (400.0, 300.0, 1.0),  // both axes at once -> the double-baked corner
        (120.0, 95.0, 1.0),   // back the other way: negative scroll
        (120.0, 95.0, 0.55),  // fade only: layer rebuilds, sprite must not
        (120.0, 95.0, 0.0),   // clouds off entirely
        (120.0, 95.0, 1.0),   // and back on
        (90000.0, -4000.0, 1.0), // fling: no overlap, full rebuild
        (90011.0, -4000.0, 1.0), // and it scrolls correctly from there
    ];

    #[test]
    fn cached_clouds_match_a_fresh_paint() {
        // Deliberately not a multiple of `cell`, so the partial right/bottom
        // cells are covered.
        agrees_with_uncached(&CLOUDY, 137, 83, PANS);
    }

    #[test]
    fn cached_plain_ground_matches_a_fresh_paint() {
        agrees_with_uncached(&PLAIN, 137, 83, PANS);
    }

    /// Paint `seed`'s clouds over the SAME patch of world as every other seed,
    /// by panning back exactly as far as its own drift offset carries it, and
    /// return which px came out cloudy.
    ///
    /// The mask is what matters: it depends on the density field alone, so it
    /// ignores the two tints the seed also picks. Two seeds agreeing here means
    /// they are drawing one shape in two colours.
    fn cloud_mask(seed: u32, w: u32, h: u32) -> Vec<bool> {
        let neb = CLOUDY.nebula.unwrap();
        // Undo `paint_backdrop`'s seeded offset: it adds hash3(si,5,2)*500 px to
        // the drift, and a pan reaches the clouds scaled down by `scroll`.
        let back = |salt| -(hash3(seed as i32, salt, 2) * 500.0) / neb.scroll;
        let mut buf = vec![0u8; (w * h * 4) as usize];
        paint_backdrop(&mut buf, w, h, &CLOUDY, seed, back(5), back(6), 1.0, 1.0, None);
        // Base navy sums to 30; its dither swings ±6. 45 clears both.
        buf.chunks_exact(4).map(|p| p[0] as u32 + p[1] as u32 + p[2] as u32 > 45).collect()
    }

    /// The seed has to change the SHAPE of the clouds, not just where they sit.
    ///
    /// This is a real bug caught late: the nebula's fBm is 3D, but both calls
    /// pinned z to a constant, so every system in the game sampled one identical
    /// plane. The seed only chose two tints and slid the sampling window by up to
    /// 500 px — less than a viewport — so two systems could share ~90% of their
    /// clouds. It looked seeded, because the colours differed.
    ///
    /// Comparing masks at a shared world position is what makes that visible: it
    /// takes the tints and the offset out of the picture and leaves only the
    /// field. Under the old code every pair below agreed on ~100% of px.
    #[test]
    fn the_seed_changes_the_clouds_not_just_their_position() {
        // Big enough to hold real structure: the field runs at 1/240 px, so a
        // frame much under this spans less than one lattice cell and comes out a
        // single flat gradient — all cloud or none, regardless of seed.
        let (w, h) = (900, 560);
        let seeds = [3u32, 7, 21, 42];
        let masks: Vec<_> = seeds.iter().map(|&s| cloud_mask(s, w, h)).collect();
        let n = (w * h) as f32;
        for i in 0..seeds.len() {
            // A seed must also actually draw clouds, or two blank skies would
            // "differ" by 0% and pass the pair check below for the wrong reason.
            let cover = masks[i].iter().filter(|&&c| c).count() as f32 / n;
            assert!(
                (0.02..0.98).contains(&cover),
                "seed {} covers {:.0}% of the frame — nearly blank or nearly solid",
                seeds[i],
                cover * 100.0
            );
            for j in i + 1..seeds.len() {
                let differ = masks[i].iter().zip(&masks[j]).filter(|(a, b)| a != b).count() as f32 / n;
                assert!(
                    differ > 0.15,
                    "seeds {} and {} draw the same cloud shape at the same world position \
                     ({:.1}% of px differ) — the noise plane is not seeded",
                    seeds[i],
                    seeds[j],
                    differ * 100.0
                );
            }
        }
    }

    /// A resize has no sprite to slide — it must fall back to a full rebuild
    /// rather than reusing a field of the wrong stride.
    #[test]
    fn resizing_rebuilds_rather_than_reusing() {
        let cache = RefCell::new(BackdropCache::default());
        for &(w, h) in &[(137u32, 83u32), (64, 200), (137, 83), (301, 41)] {
            let len = (w * h * 4) as usize;
            let (mut cached, mut fresh) = (vec![0u8; len], vec![0u8; len]);
            for &(bgx, bgy, amt) in PANS {
                paint_backdrop(&mut cached, w, h, &CLOUDY, 7, bgx, bgy, 1.0, amt, Some(&cache));
                paint_backdrop(&mut fresh, w, h, &CLOUDY, 7, bgx, bgy, 1.0, amt, None);
                assert_eq!(cached, fresh, "{w}x{h} after resize, pan ({bgx}, {bgy})");
            }
        }
    }
}
