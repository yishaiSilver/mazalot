//! Native planet generator: turns `planet-core`'s RGBA frames into GIFs, a
//! contact-sheet PNG, and a combined all-types GIF.
//!
//! All planet math + the 26-type table live in the `planet-core` crate (shared
//! with the web/WASM build). This file is only the `image`-crate orchestration.

use image::codecs::gif::{GifEncoder, Repeat};
use image::{imageops, Delay, Frame, Rgba, RgbaImage};
use planet_core::{render_rgba, type_count, type_name};
use std::f32::consts::TAU;
use std::fs::File;

const NATIVE: u32 = 64; // render resolution (px)
const FRAMES: usize = 30; // frames per full rotation
const GIF_UP: u32 = 3; // nearest-neighbour zoom for individual GIFs
const POSTER_UP: u32 = 2; // zoom for the contact-sheet PNG

/// One native-resolution frame via the shared core.
fn render_frame(type_idx: usize, seed: u32, angle: f32) -> RgbaImage {
    let mut buf = vec![0u8; (NATIVE * NATIVE * 4) as usize];
    render_rgba(NATIVE, type_idx, seed, angle, &mut buf);
    RgbaImage::from_raw(NATIVE, NATIVE, buf).expect("buffer size matches")
}

fn zoom(img: &RgbaImage, s: u32) -> RgbaImage {
    imageops::resize(img, img.width() * s, img.height() * s, imageops::FilterType::Nearest)
}

fn write_gif(path: &str, type_idx: usize, seed: u32) -> Result<(), Box<dyn std::error::Error>> {
    let file = File::create(path)?;
    let mut enc = GifEncoder::new(file);
    enc.set_repeat(Repeat::Infinite)?;
    for f in 0..FRAMES {
        let angle = TAU * (f as f32) / (FRAMES as f32);
        let frame = zoom(&render_frame(type_idx, seed, angle), GIF_UP);
        enc.encode_frame(Frame::from_parts(frame, 0, 0, Delay::from_numer_denom_ms(70, 1)))?;
    }
    Ok(())
}

/// One animated GIF where every type spins together in a grid.
fn write_combined_gif(path: &str, count: usize, cols: u32) -> Result<(), Box<dyn std::error::Error>> {
    let gut = 2u32;
    let cw = NATIVE + gut;
    let rows = (count as u32 + cols - 1) / cols;
    let file = File::create(path)?;
    let mut enc = GifEncoder::new(file);
    enc.set_repeat(Repeat::Infinite)?;
    for f in 0..FRAMES {
        let angle = TAU * (f as f32) / (FRAMES as f32);
        let mut grid = RgbaImage::new(gut + cols * cw, gut + rows * cw);
        for px in grid.pixels_mut() {
            *px = Rgba([6, 6, 14, 255]);
        }
        for i in 0..count {
            let cell = render_frame(i, 100 + i as u32 * 13, angle);
            let x = gut + (i as u32 % cols) * cw;
            let y = gut + (i as u32 / cols) * cw;
            imageops::overlay(&mut grid, &cell, x as i64, y as i64);
        }
        let frame = zoom(&grid, 2);
        enc.encode_frame(Frame::from_parts(frame, 0, 0, Delay::from_numer_denom_ms(70, 1)))?;
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("out")?;
    let count = type_count();

    // 1) one spinning GIF per type
    for i in 0..count {
        let path = format!("out/planet_{}.gif", type_name(i));
        write_gif(&path, i, 100 + i as u32 * 13)?;
        println!("Wrote {path}");
    }

    // 1b) all types spinning together
    write_combined_gif("out/planets_all.gif", count, 6)?;
    println!("Wrote out/planets_all.gif");

    // 2) aggregate table PNG: one row per type, several seeds across
    let cols = 6u32;
    let gut = 3u32;
    let cw = NATIVE + gut;
    let rows = count as u32;
    let mut table = RgbaImage::new(gut + cols * cw, gut + rows * cw);
    for px in table.pixels_mut() {
        *px = Rgba([6, 6, 14, 255]);
    }
    for r in 0..count {
        for c in 0..cols {
            let seed = (r as u32) * 100 + c * 7 + 1;
            let angle = 0.5 + c as f32 * 0.32;
            let cell = render_frame(r, seed, angle);
            let x = gut + c * cw;
            let y = gut + r as u32 * cw;
            imageops::overlay(&mut table, &cell, x as i64, y as i64);
        }
    }
    zoom(&table, POSTER_UP).save("out/planets_table.png")?;
    println!("Wrote out/planets_table.png ({} types x {} seeds = {} planets)", count, cols, count as u32 * cols);
    Ok(())
}
