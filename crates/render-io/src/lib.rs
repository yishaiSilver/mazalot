//! render-io — the native `image`-crate orchestration shared by the space-crate
//! bins: turning RGBA frames into GIFs, contact-sheet PNGs, and orbit/pan
//! animations. Two families:
//!   * "spinning body" (planet, star): [`write_spin_gif`], [`write_spin_grid_gif`],
//!     [`write_contact_sheet`].
//!   * "scene / camera" (solar, moon, comet, asteroid): [`fit_zoom`],
//!     [`write_orbit_gif`], [`write_anim_gif`], [`write_poster`].
//!
//! The per-frame render call and the (per-crate) `Camera` stay in each bin's
//! closure; this crate only owns the `image`-crate calls. Every helper issues
//! those calls in the exact order/params the bins used to, so output bytes are
//! unchanged (e.g. angle/`t` keep the `X * f as f32 / n as f32` associativity,
//! and each frame gets a fresh buffer).

use image::imageops;
use rayon::prelude::*;
use std::error::Error;
use std::f32::consts::TAU;
use std::fs::File;

// Re-exported so bins need no direct `image` dependency.
pub use image::{Rgba, RgbaImage};

type Res = Result<(), Box<dyn Error>>;

/// `image`'s `GifEncoder::new` quality setting. 1 is "prioritize quality over
/// performance at any cost", and it is most of this crate's runtime — see
/// `encode_gif`.
const GIF_SPEED: i32 = 1;

/// Nearest-neighbour integer upscale (the bins' old `zoom`).
pub fn upscale_nearest(img: &RgbaImage, s: u32) -> RgbaImage {
    imageops::resize(img, img.width() * s, img.height() * s, imageops::FilterType::Nearest)
}

/// Encode an ordered sequence of frames as an infinite-repeat GIF.
///
/// This drives the `gif` crate directly rather than going through
/// `image::codecs::gif::GifEncoder`, for one reason: the encoder is a single
/// stateful writer, but the expensive half of each frame — NeuQuant palette
/// quantization, at `speed = 1` — is a *pure function of that frame*. Splitting
/// them lets the quantization run across every core while the writes stay
/// ordered and serial. It is worth the trouble: GIF encoding was ~85% of the
/// native generators' runtime, and one file (planet's all-types grid) was 19s
/// of a 30s run on its own.
///
/// The steps below mirror `GifEncoder::convert_frame`/`encode_gif` exactly —
/// same speed, same `delay / 10` integer truncation, same `Background`
/// disposal, same empty global palette sized from the first frame, `set_repeat`
/// before any frame. That correspondence is what keeps every byte of `out/`
/// unchanged, so if you touch this, re-check the hashes rather than eyeballing
/// a GIF.
fn encode_gif(path: &str, delay_ms: u32, frames: Vec<RgbaImage>) -> Res {
    let quantized: Vec<gif::Frame<'static>> = frames
        .into_par_iter()
        .map(|img| {
            let (w, h) = (img.width() as u16, img.height() as u16);
            let mut raw = img.into_raw();
            let mut f = gif::Frame::from_rgba_speed(w, h, &mut raw, GIF_SPEED);
            // `image` builds a `Delay` from (delay_ms, 1) and then truncates to
            // GIF's 10ms ticks. Integer division, so 70ms -> 7, same as before.
            f.delay = (delay_ms / 10) as u16;
            f.dispose = gif::DisposalMethod::Background;
            f
        })
        .collect();

    let file = File::create(path)?;
    // No frames: `image` would leave the freshly-created file empty too.
    let Some(first) = quantized.first() else { return Ok(()) };
    let mut enc = gif::Encoder::new(file, first.width, first.height, &[])?;
    enc.set_repeat(gif::Repeat::Infinite)?;
    for f in &quantized {
        enc.write_frame(f)?;
    }
    Ok(())
}

/// Compose `count` cells `cols` wide over a solid `background`, cell size taken
/// from the produced images, `gutter` px between/around. Reproduces both the
/// combined-GIF grid and the contact-sheet layout exactly (`cw = cell + gutter`,
/// canvas `gutter + cols*cw` by `gutter + rows*cw`).
fn compose_grid<F: Fn(usize) -> RgbaImage + Sync>(
    count: usize,
    cols: u32,
    gutter: u32,
    background: [u8; 4],
    cell: F,
) -> RgbaImage {
    // Cells are independent; the overlay below is not, so it stays in order.
    // `&cell`, not `cell`: mapping the reference means callers only owe us
    // `Sync`, where moving the closure in would demand `Send` as well.
    let cells: Vec<RgbaImage> = (0..count).into_par_iter().map(&cell).collect();
    let cw = cells[0].width() + gutter;
    let rows = (count as u32 + cols - 1) / cols;
    let mut grid = RgbaImage::new(gutter + cols * cw, gutter + rows * cw);
    for px in grid.pixels_mut() {
        *px = Rgba(background);
    }
    for (i, c) in cells.iter().enumerate() {
        let x = gutter + (i as u32 % cols) * cw;
        let y = gutter + (i as u32 / cols) * cw;
        imageops::overlay(&mut grid, c, x as i64, y as i64);
    }
    grid
}

// ---------------------------------------------------------------------------
// Family A: spinning bodies
// ---------------------------------------------------------------------------

/// One spinning-body GIF. Sweeps `angle = TAU * f / frames` internally, upscales
/// each frame by `upscale` (nearest), encodes infinite-repeat at `delay_ms`.
pub fn write_spin_gif<F: Fn(f32) -> RgbaImage + Sync>(
    path: &str,
    frames: usize,
    delay_ms: u32,
    upscale: u32,
    render: F,
) -> Res {
    let imgs: Vec<RgbaImage> = (0..frames)
        .into_par_iter()
        .map(|f| {
            // `angle` is still computed from `f` alone, with the original
            // associativity — frames stay independent, so running them out of
            // order changes nothing.
            let angle = TAU * (f as f32) / (frames as f32);
            upscale_nearest(&render(angle), upscale)
        })
        .collect();
    encode_gif(path, delay_ms, imgs)
}

/// The "all types spinning together" grid GIF. `count` cells `cols` wide over
/// `background`, whole grid upscaled by `grid_upscale` per frame.
pub fn write_spin_grid_gif<F: Fn(usize, f32) -> RgbaImage + Sync>(
    path: &str,
    count: usize,
    cols: u32,
    frames: usize,
    delay_ms: u32,
    gutter: u32,
    background: [u8; 4],
    grid_upscale: u32,
    render: F,
) -> Res {
    let imgs: Vec<RgbaImage> = (0..frames)
        .into_par_iter()
        .map(|f| {
            let angle = TAU * (f as f32) / (frames as f32);
            let grid = compose_grid(count, cols, gutter, background, |i| render(i, angle));
            upscale_nearest(&grid, grid_upscale)
        })
        .collect();
    encode_gif(path, delay_ms, imgs)
}

/// The contact-sheet PNG: a `rows` × `cols` grid of stills, upscaled + saved.
/// `cell(row, col)` produces each cell.
pub fn write_contact_sheet<F: Fn(u32, u32) -> RgbaImage + Sync>(
    path: &str,
    rows: u32,
    cols: u32,
    gutter: u32,
    background: [u8; 4],
    upscale: u32,
    cell: F,
) -> Res {
    let count = (rows * cols) as usize;
    let grid = compose_grid(count, cols, gutter, background, |i| {
        cell(i as u32 / cols, i as u32 % cols)
    });
    upscale_nearest(&grid, upscale).save(path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Family B: scene / camera
// ---------------------------------------------------------------------------

/// Pure fit-to-viewport zoom. Reproduces each crate's `fit_zoom`:
/// solar `(0.92, 0.55)`, asteroid `(0.92, 0.50)`, comet `(0.90, 0.60)`,
/// moon `(0.92, 0.60)`.
pub fn fit_zoom(ext: f32, w: u32, h: u32, margin: f32, vsquash: f32) -> f32 {
    let halfw = w as f32 * 0.5 * margin;
    let halfh = h as f32 * 0.5 * margin;
    (halfw / ext).min(halfh / (ext * vsquash))
}

/// Orbit/animation GIF. Sweeps `t = span * f / frames` internally (same
/// associativity as the originals) and wraps each returned buffer as an image.
/// The closure gets `(w, h, t)` and returns a fresh `w*h*4` RGBA buffer.
///
/// Frames here render **serially**, unlike the spinning-body family above. The
/// scene crates hand this a closure borrowing their `System`/`Belt`/`Scene`,
/// which carries `RefCell` caches (draw order, the baked sun tile, the nebula)
/// and so is deliberately not `Sync`. Parallelising this would mean either a
/// system per thread — losing exactly the caches that make a scene frame cheap —
/// or making them thread-safe, which the wasm demos pay for and do not use.
/// `encode_gif` is still parallel, and encoding is the bulk of the cost.
pub fn write_orbit_gif<F: FnMut(u32, u32, f32) -> Vec<u8>>(
    path: &str,
    w: u32,
    h: u32,
    frames: usize,
    span: f32,
    delay_ms: u32,
    mut render: F,
) -> Res {
    let mut imgs = Vec::with_capacity(frames);
    for f in 0..frames {
        let t = span * f as f32 / frames as f32;
        let buf = render(w, h, t);
        imgs.push(RgbaImage::from_raw(w, h, buf).expect("buffer size matches"));
    }
    encode_gif(path, delay_ms, imgs)
}

/// General GIF driver for bespoke per-frame motion (the pan GIFs). The closure
/// gets `(f, frames, w, h)` so it can compute its own easing with the original
/// float expressions untouched, and returns a fresh `w*h*4` RGBA buffer.
///
/// Serial for the same reason as [`write_orbit_gif`]; the encode is parallel.
pub fn write_anim_gif<F: FnMut(usize, usize, u32, u32) -> Vec<u8>>(
    path: &str,
    w: u32,
    h: u32,
    frames: usize,
    delay_ms: u32,
    mut render: F,
) -> Res {
    let mut imgs = Vec::with_capacity(frames);
    for f in 0..frames {
        let buf = render(f, frames, w, h);
        imgs.push(RgbaImage::from_raw(w, h, buf).expect("buffer size matches"));
    }
    encode_gif(path, delay_ms, imgs)
}

/// Single still PNG at a fixed `t`. Scene reporting stays in the caller.
pub fn write_poster<F: FnOnce(u32, u32, f32) -> Vec<u8>>(path: &str, w: u32, h: u32, t: f32, render: F) -> Res {
    let buf = render(w, h, t);
    RgbaImage::from_raw(w, h, buf).expect("buffer size matches").save(path)?;
    Ok(())
}
