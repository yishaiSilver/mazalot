//! Native spaceship generator: turns the `ship` crate's RGBA frames into a
//! turning-hull GIF, a burn GIF, per-role contact sheets, an all-class
//! poster, a true-relative-scale lineup and a one-class/many-seeds variant
//! sheet.
//!
//! All generation + render math lives in the `ship` crate (shared with the
//! web/WASM build). This file is only the `image`-crate orchestration — the
//! same spirit as `planet`'s and `comet`'s native bins.

use image::codecs::gif::{GifEncoder, Repeat};
use image::{imageops, Delay, Frame, Rgba, RgbaImage};
use ship::{
    class_count, class_length_m, class_name, classes_in_role, role_count, role_name, Ship, View,
};
use std::f32::consts::TAU;
use std::fs::File;

const CELL_W: u32 = 92; // contact-sheet cell (ships are tall: nose-up plan view)
const CELL_H: u32 = 124;
const UP: u32 = 2; // nearest-neighbour zoom for the posters
const BG: Rgba<u8> = Rgba([7, 8, 16, 255]);

/// Render one ship into its own transparent-background RGBA image, fitted with
/// room astern so the drive plume isn't guillotined by the cell edge.
fn cell(s: &Ship, w: u32, h: u32, t: f32) -> RgbaImage {
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let (zoom, pan_y) = s.fit_with_plume(w, h, 0.24);
    let v = View { zoom, pan_y, thrust: 0.85, stars: 0.0, ..View::default() };
    s.render(w, h, &v, t, &mut buf);
    RgbaImage::from_raw(w, h, buf).expect("buffer size matches")
}

/// Render at an explicit zoom (for the true-scale lineup), transparent bg.
fn cell_at(s: &Ship, w: u32, h: u32, zoom: f32, t: f32) -> RgbaImage {
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let v = View { zoom, thrust: 0.85, stars: 0.0, ..View::default() };
    s.render(w, h, &v, t, &mut buf);
    RgbaImage::from_raw(w, h, buf).expect("buffer size matches")
}

fn grid(cols: u32, rows: u32, cw: u32, ch: u32, gut: u32) -> RgbaImage {
    let mut g = RgbaImage::new(gut + cols * (cw + gut), gut + rows * (ch + gut));
    for px in g.pixels_mut() {
        *px = BG;
    }
    g
}

fn zoom_up(img: &RgbaImage, s: u32) -> RgbaImage {
    imageops::resize(img, img.width() * s, img.height() * s, imageops::FilterType::Nearest)
}

/// A contact sheet over an explicit list of class indices.
fn sheet(path: &str, classes: &[usize], cols: u32, seed0: u32) -> Result<(), Box<dyn std::error::Error>> {
    let gut = 3u32;
    let rows = (classes.len() as u32).div_ceil(cols);
    let mut g = grid(cols, rows, CELL_W, CELL_H, gut);
    for (n, &ci) in classes.iter().enumerate() {
        let s = Ship::generate(ci, seed0 + n as u32 * 37 + 11);
        let img = cell(&s, CELL_W, CELL_H, 0.4);
        let x = gut + (n as u32 % cols) * (CELL_W + gut);
        let y = gut + (n as u32 / cols) * (CELL_H + gut);
        imageops::overlay(&mut g, &img, x as i64, y as i64);
    }
    zoom_up(&g, UP).save(path)?;
    Ok(())
}

/// A 360° turn under a rotation-stable camera, so the hull never pulses.
fn turn_gif(path: &str, ci: usize, seed: u32, w: u32, h: u32, frames: usize) -> Result<(), Box<dyn std::error::Error>> {
    let s = Ship::generate(ci, seed);
    let zoom = s.fit_zoom_spin(w, h);
    let file = File::create(path)?;
    let mut enc = GifEncoder::new(file);
    enc.set_repeat(Repeat::Infinite)?;
    for f in 0..frames {
        let t = f as f32 / frames as f32;
        let v = View { zoom, heading: TAU * t, thrust: 0.9, ..View::default() };
        let mut buf = vec![0u8; (w * h * 4) as usize];
        s.render(w, h, &v, t * 6.0, &mut buf);
        let img = RgbaImage::from_raw(w, h, buf).expect("buffer size matches");
        enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(70, 1)))?;
    }
    Ok(())
}

/// A throttle-up/throttle-down loop on a fixed heading — all plume, no motion.
fn burn_gif(path: &str, ci: usize, seed: u32, w: u32, h: u32, frames: usize) -> Result<(), Box<dyn std::error::Error>> {
    let s = Ship::generate(ci, seed);
    // Leave the bottom 40% of the frame clear for the exhaust.
    let (zoom, pan_y) = s.fit_with_plume(w, h, 0.40);
    let file = File::create(path)?;
    let mut enc = GifEncoder::new(file);
    enc.set_repeat(Repeat::Infinite)?;
    for f in 0..frames {
        let t = f as f32 / frames as f32;
        // one smooth up-and-down burn so the loop closes seamlessly
        let thrust = 0.25 + 1.35 * (0.5 - 0.5 * (TAU * t).cos());
        let v = View { zoom, pan_y, heading: 0.0, thrust, ..View::default() };
        let mut buf = vec![0u8; (w * h * 4) as usize];
        s.render(w, h, &v, t * 8.0, &mut buf);
        let img = RgbaImage::from_raw(w, h, buf).expect("buffer size matches");
        enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(70, 1)))?;
    }
    Ok(())
}

/// A lineup at TRUE relative scale — every hull drawn at the same metres per
/// pixel, so a corvette really is a tenth of a dreadnought.
fn lineup(path: &str, classes: &[usize], w: u32, h: u32) -> Result<(), Box<dyn std::error::Error>> {
    let ships: Vec<Ship> =
        classes.iter().enumerate().map(|(i, &c)| Ship::generate(c, 500 + i as u32 * 91)).collect();
    // `zoom` is px per ship-LENGTH unit and one length unit *is* `length_m`, so
    // a single metres-per-pixel scale for the whole row is just ppm*length_m.
    // Size it so the tallest hull fits, then shrink again if the row is too wide
    // (a dreadnought is half as wide as it is long and would overrun its slot).
    let tallest = ships
        .iter()
        .fold(1e-4f32, |a, s| {
            let (_, _, y0, y1) = s.bounds();
            a.max(s.length_m * (y1 - y0))
        });
    let mut ppm = (h as f32 * 0.86) / tallest;
    let gut = 10.0f32;
    let width_at = |ppm: f32, s: &Ship| {
        let (x0, x1, _, _) = s.bounds();
        (ppm * s.length_m * (x1 - x0)).ceil().max(4.0) + gut
    };
    let total: f32 = ships.iter().map(|s| width_at(ppm, s)).sum();
    if total > w as f32 {
        ppm *= (w as f32 - gut * ships.len() as f32) / (total - gut * ships.len() as f32);
    }
    let mut g = RgbaImage::new(w, h);
    for px in g.pixels_mut() {
        *px = BG;
    }
    // Centre the row horizontally, each hull vertically inside its own slot.
    let widths: Vec<f32> = ships.iter().map(|s| width_at(ppm, s)).collect();
    let mut x = (w as f32 - widths.iter().sum::<f32>()).max(0.0) * 0.5;
    for (s, cw) in ships.iter().zip(&widths) {
        let img = cell_at(s, *cw as u32, h, ppm * s.length_m, 0.4);
        imageops::overlay(&mut g, &img, x as i64, 0);
        x += cw;
    }
    g.save(path)?;
    Ok(())
}

fn find(name: &str) -> usize {
    (0..class_count()).find(|&i| class_name(i) == name).unwrap_or(0)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("out")?;
    let n = class_count();

    // 1) the headline: a heavy cruiser making a slow 360.
    let hero = find("heavy_cruiser");
    turn_gif("out/ship.gif", hero, 31, 240, 240, 48)?;
    println!("Wrote out/ship.gif ({})", Ship::generate(hero, 31).designation());

    // 2) a burn loop — the plume doing the work.
    let burner = find("destroyer");
    burn_gif("out/ship_burn.gif", burner, 7, 200, 280, 40)?;
    println!("Wrote out/ship_burn.gif ({})", Ship::generate(burner, 7).designation());

    // 3) every class on one poster.
    sheet("out/ships_all.png", &(0..n).collect::<Vec<_>>(), 9, 100)?;
    println!("Wrote out/ships_all.png ({n} classes)");

    // 4) one poster per role.
    for r in 0..role_count() {
        let ids = classes_in_role(r);
        let cols = (ids.len() as u32).clamp(1, 7);
        let path = format!("out/ships_{}.png", role_name(r));
        sheet(&path, &ids, cols, 200 + r as u32 * 13)?;
        println!("Wrote {path} ({} classes)", ids.len());
    }

    // 5) one class, many seeds — proof that a class is a family, not a ship.
    let vari = find("bulk_freighter");
    sheet("out/ship_variants.png", &[vari; 24], 8, 3000)?;
    println!("Wrote out/ship_variants.png (24 seeds of {})", class_name(vari));

    // 6) a true-relative-scale lineup across two orders of hull size.
    let picks: Vec<usize> = ["shuttle", "gunship", "corvette", "frigate", "destroyer", "heavy_cruiser", "fleet_carrier", "dreadnought"]
        .iter()
        .map(|w| find(w))
        .collect();
    lineup("out/ships_lineup.png", &picks, 1120, 460)?;
    println!("Wrote out/ships_lineup.png (true relative scale)");
    for &c in &picks {
        println!("    {:>14}  {:>7.0} m", class_name(c), class_length_m(c));
    }

    // 7) report what the table holds.
    println!("\n{n} classes across {} roles:", role_count());
    for r in 0..role_count() {
        let ids = classes_in_role(r);
        let names: Vec<&str> = ids.iter().map(|&i| class_name(i)).collect();
        println!("  {:>11} ({:>2}): {}", role_name(r), ids.len(), names.join(", "));
    }
    Ok(())
}
