//! Style-comparison generator: renders every [`StarPattern`] so the alternative
//! looks can be evaluated side-by-side against the original `Realistic` sun.
//!
//! Outputs into `out/styles/`:
//!   • `<pattern>_<type>.gif`      — one spinning GIF per (pattern, showcase type)
//!   • `styles_grid.gif`           — animated grid: rows = patterns, cols = types
//!   • `styles_contact.png`        — static contact sheet of the same grid

use image::codecs::gif::{GifEncoder, Repeat};
use image::{imageops, Delay, Frame, Rgba, RgbaImage};
use star::{render_pattern, type_name, StarPattern};
use std::f32::consts::TAU;
use std::fs::File;

const NATIVE: u32 = 80;
const FRAMES: usize = 30;
const GIF_UP: u32 = 3;

/// A few representative star types to show each pattern against.
const SHOWCASE: &[usize] = &[2, 4, 0, 7]; // yellow_dwarf, red_giant, blue_giant, sol

fn render_frame(type_idx: usize, seed: u32, angle: f32, pat: StarPattern) -> RgbaImage {
    let mut buf = vec![0u8; (NATIVE * NATIVE * 4) as usize];
    render_pattern(NATIVE, type_idx, seed, angle, pat, &mut buf);
    RgbaImage::from_raw(NATIVE, NATIVE, buf).expect("buffer size matches")
}

fn zoom(img: &RgbaImage, s: u32) -> RgbaImage {
    imageops::resize(img, img.width() * s, img.height() * s, imageops::FilterType::Nearest)
}

fn write_gif(path: &str, type_idx: usize, seed: u32, pat: StarPattern) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::create(path)?;
    let mut enc = GifEncoder::new(file);
    enc.set_repeat(Repeat::Infinite)?;
    for f in 0..FRAMES {
        let angle = TAU * (f as f32) / (FRAMES as f32);
        let frame = zoom(&render_frame(type_idx, seed, angle, pat), GIF_UP);
        enc.encode_frame(Frame::from_parts(frame, 0, 0, Delay::from_numer_denom_ms(70, 1)))?;
    }
    Ok(())
}

/// Animated grid: one row per pattern, one column per showcase star type.
fn write_grid_gif(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let gut = 2u32;
    let cw = NATIVE + gut;
    let cols = SHOWCASE.len() as u32;
    let rows = StarPattern::ALL.len() as u32;
    let file = File::create(path)?;
    let mut enc = GifEncoder::new(file);
    enc.set_repeat(Repeat::Infinite)?;
    for f in 0..FRAMES {
        let angle = TAU * (f as f32) / (FRAMES as f32);
        let mut grid = RgbaImage::new(gut + cols * cw, gut + rows * cw);
        for px in grid.pixels_mut() {
            *px = Rgba([6, 6, 14, 255]);
        }
        for (ri, &pat) in StarPattern::ALL.iter().enumerate() {
            for (ci, &ty) in SHOWCASE.iter().enumerate() {
                let cell = render_frame(ty, 100 + ci as u32 * 13, angle, pat);
                let x = gut + ci as u32 * cw;
                let y = gut + ri as u32 * cw;
                imageops::overlay(&mut grid, &cell, x as i64, y as i64);
            }
        }
        enc.encode_frame(Frame::from_parts(zoom(&grid, 2), 0, 0, Delay::from_numer_denom_ms(70, 1)))?;
    }
    Ok(())
}

/// Static contact sheet (single frame) of the same pattern×type grid.
fn write_contact(path: &str) -> Result<(), Box<dyn std::error::Error>> {
    let gut = 3u32;
    let cw = NATIVE + gut;
    let cols = SHOWCASE.len() as u32;
    let rows = StarPattern::ALL.len() as u32;
    let mut sheet = RgbaImage::new(gut + cols * cw, gut + rows * cw);
    for px in sheet.pixels_mut() {
        *px = Rgba([6, 6, 14, 255]);
    }
    for (ri, &pat) in StarPattern::ALL.iter().enumerate() {
        for (ci, &ty) in SHOWCASE.iter().enumerate() {
            let cell = render_frame(ty, 100 + ci as u32 * 13, 1.1, pat);
            let x = gut + ci as u32 * cw;
            let y = gut + ri as u32 * cw;
            imageops::overlay(&mut sheet, &cell, x as i64, y as i64);
        }
    }
    zoom(&sheet, 2).save(path)?;
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("out/styles")?;

    // One spinning GIF per pattern on the flagship yellow-dwarf sun.
    for &pat in StarPattern::ALL.iter() {
        let path = format!("out/styles/{}_yellow_dwarf.gif", pat.name());
        write_gif(&path, 2, 113, pat)?;
        println!("Wrote {path}");
    }

    write_grid_gif("out/styles/styles_grid.gif")?;
    println!("Wrote out/styles/styles_grid.gif (rows: {} patterns, cols: {} types)",
        StarPattern::ALL.len(), SHOWCASE.len());

    write_contact("out/styles/styles_contact.png")?;
    println!("Wrote out/styles/styles_contact.png");

    let names: Vec<_> = SHOWCASE.iter().map(|&t| type_name(t)).collect();
    println!("Showcase types (columns): {names:?}");
    Ok(())
}
