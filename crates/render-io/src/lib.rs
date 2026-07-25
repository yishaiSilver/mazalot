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

use image::codecs::gif::{GifEncoder, Repeat};
use image::{imageops, Delay, Frame};
use std::error::Error;
use std::f32::consts::TAU;
use std::fs::File;

// Re-exported so bins need no direct `image` dependency.
pub use image::{Rgba, RgbaImage};

type Res = Result<(), Box<dyn Error>>;

/// Nearest-neighbour integer upscale (the bins' old `zoom`).
pub fn upscale_nearest(img: &RgbaImage, s: u32) -> RgbaImage {
    imageops::resize(img, img.width() * s, img.height() * s, imageops::FilterType::Nearest)
}

/// Encode an ordered sequence of frames as an infinite-repeat GIF.
fn encode_gif<I: IntoIterator<Item = RgbaImage>>(path: &str, delay_ms: u32, frames: I) -> Res {
    let file = File::create(path)?;
    let mut enc = GifEncoder::new(file);
    enc.set_repeat(Repeat::Infinite)?;
    for img in frames {
        enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(delay_ms, 1)))?;
    }
    Ok(())
}

/// Compose `count` cells `cols` wide over a solid `background`, cell size taken
/// from the produced images, `gutter` px between/around. Reproduces both the
/// combined-GIF grid and the contact-sheet layout exactly (`cw = cell + gutter`,
/// canvas `gutter + cols*cw` by `gutter + rows*cw`).
fn compose_grid<F: FnMut(usize) -> RgbaImage>(
    count: usize,
    cols: u32,
    gutter: u32,
    background: [u8; 4],
    mut cell: F,
) -> RgbaImage {
    let cells: Vec<RgbaImage> = (0..count).map(|i| cell(i)).collect();
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
pub fn write_spin_gif<F: FnMut(f32) -> RgbaImage>(
    path: &str,
    frames: usize,
    delay_ms: u32,
    upscale: u32,
    mut render: F,
) -> Res {
    let mut imgs = Vec::with_capacity(frames);
    for f in 0..frames {
        let angle = TAU * (f as f32) / (frames as f32);
        imgs.push(upscale_nearest(&render(angle), upscale));
    }
    encode_gif(path, delay_ms, imgs)
}

/// The "all types spinning together" grid GIF. `count` cells `cols` wide over
/// `background`, whole grid upscaled by `grid_upscale` per frame.
pub fn write_spin_grid_gif<F: FnMut(usize, f32) -> RgbaImage>(
    path: &str,
    count: usize,
    cols: u32,
    frames: usize,
    delay_ms: u32,
    gutter: u32,
    background: [u8; 4],
    grid_upscale: u32,
    mut render: F,
) -> Res {
    let mut imgs = Vec::with_capacity(frames);
    for f in 0..frames {
        let angle = TAU * (f as f32) / (frames as f32);
        let grid = compose_grid(count, cols, gutter, background, |i| render(i, angle));
        imgs.push(upscale_nearest(&grid, grid_upscale));
    }
    encode_gif(path, delay_ms, imgs)
}

/// The contact-sheet PNG: a `rows` × `cols` grid of stills, upscaled + saved.
/// `cell(row, col)` produces each cell.
pub fn write_contact_sheet<F: FnMut(u32, u32) -> RgbaImage>(
    path: &str,
    rows: u32,
    cols: u32,
    gutter: u32,
    background: [u8; 4],
    upscale: u32,
    mut cell: F,
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
