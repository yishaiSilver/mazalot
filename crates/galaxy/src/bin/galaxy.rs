//! Native galaxy generator: turns the `galaxy` crate's RGBA map into framed
//! poster PNGs and a slow fly-across GIF. All layout + render math lives in the
//! `galaxy` crate (shared with the web/WASM build); this file is only the
//! `image`-crate orchestration, the same spirit as solar's/star's native bins.

use galaxy::{region_name, Camera, Galaxy};
use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame, RgbaImage};
use std::fs::File;

/// Render one map frame straight into an `RgbaImage`.
fn frame(gal: &mut Galaxy, w: u32, h: u32, cam: &Camera, t: f32, sel: i32) -> RgbaImage {
    let mut buf = vec![0u8; (w * h * 4) as usize];
    galaxy::render_map_cached(gal, w, h, cam, t, sel, -1, &mut buf);
    RgbaImage::from_raw(w, h, buf).expect("buffer size matches")
}

/// Zoom that fits the whole galaxy into a `w`x`h` viewport with margin.
fn fit_zoom(gal: &Galaxy, w: u32, h: u32) -> f32 {
    let ext = gal.extent();
    (0.46 * w as f32 / ext).min(0.46 * h as f32 / ext)
}

/// A framed poster still of the whole galaxy, and a one-line seed summary.
fn write_poster(path: &str, seed: u32, w: u32, h: u32) -> Result<(), Box<dyn std::error::Error>> {
    let mut gal = Galaxy::generate(seed);
    let cam = Camera { x: 0.0, y: 0.0, zoom: fit_zoom(&gal, w, h) };
    frame(&mut gal, w, h, &cam, 3.0, -1).save(path)?;
    // Report what the seed produced.
    let mut region_hits = [0usize; 8];
    for n in &gal.nodes {
        region_hits[(n.region as usize) % 8] += 1;
    }
    print!("  seed {seed}: {} systems, {} hyperlanes · regions:", gal.nodes.len(), gal.edges.len());
    for (i, &c) in region_hits.iter().enumerate() {
        if c > 0 {
            print!(" {}({})", region_name(i), c);
        }
    }
    println!();
    Ok(())
}

/// A GIF that slowly zooms from the whole galaxy down toward its core, with the
/// stars twinkling — the "fly in" beat before you pick a system.
fn write_zoom_gif(path: &str, seed: u32, w: u32, h: u32, frames: usize) -> Result<(), Box<dyn std::error::Error>> {
    let mut gal = Galaxy::generate(seed);
    let fit = fit_zoom(&gal, w, h);
    let file = File::create(path)?;
    let mut enc = GifEncoder::new(file);
    enc.set_repeat(Repeat::Infinite)?;
    for f in 0..frames {
        let u = f as f32 / frames as f32;
        // Ease in and back out (0→1→0) so the loop is seamless.
        let s = 0.5 - 0.5 * (u * std::f32::consts::TAU).cos();
        let cam = Camera { x: 0.0, y: 0.0, zoom: fit * (1.0 + 1.6 * s) };
        let img = frame(&mut gal, w, h, &cam, u * 20.0, -1);
        enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(90, 1)))?;
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("out")?;

    // 1) the headline: a fitted galaxy poster.
    println!("Posters:");
    for (i, seed) in [7u32, 3, 42, 2024].iter().enumerate() {
        write_poster(&format!("out/galaxy_{}.png", i), *seed, 1000, 1000)?;
    }

    // 2) a slow zoom-in loop.
    write_zoom_gif("out/galaxy_zoom.gif", 7, 640, 640, 60)?;
    println!("Wrote out/galaxy_zoom.gif");

    Ok(())
}
