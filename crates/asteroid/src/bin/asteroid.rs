//! Native asteroid-belt generator: turns the `asteroid` crate's RGBA scenes into
//! a revolving belt GIF, a camera-pan "drift tour" GIF, and framed poster PNGs.
//!
//! All belt math + the type tables live in the `asteroid` crate (shared with the
//! web/WASM build). This file is only the `image`-crate orchestration — the same
//! spirit as `solar`'s / `star`'s native bins, self-contained in `asteroid`.

use asteroid::{render_belt, Belt, Camera};
use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame, RgbaImage};
use std::fs::File;

/// Render one scene frame straight into an `RgbaImage`.
fn frame(belt: &Belt, w: u32, h: u32, cam: &Camera, t: f32) -> RgbaImage {
    let mut buf = vec![0u8; (w * h * 4) as usize];
    render_belt(belt, w, h, cam, t, &mut buf);
    RgbaImage::from_raw(w, h, buf).expect("buffer size matches")
}

/// Zoom that fits the whole belt into a `w`x`h` viewport with margin. The ring
/// is squashed vertically (~0.42), so height is the tighter axis; fit against
/// both with a comfortable margin.
fn fit_zoom(belt: &Belt, w: u32, h: u32) -> f32 {
    let ext = belt.extent();
    let halfw = w as f32 * 0.5 * 0.92;
    let halfh = h as f32 * 0.5 * 0.92;
    (halfw / ext).min(halfh / (ext * 0.5))
}

/// A GIF where the belt revolves under a fixed, fitted camera.
fn write_orbit_gif(path: &str, seed: u32, w: u32, h: u32, frames: usize) -> Result<(), Box<dyn std::error::Error>> {
    let belt = Belt::generate(seed);
    let cam = Camera { x: 0.0, y: 0.0, zoom: fit_zoom(&belt, w, h) };
    let file = File::create(path)?;
    let mut enc = GifEncoder::new(file);
    enc.set_repeat(Repeat::Infinite)?;
    // Sweep enough time that the inner rocks make a lap or two.
    let span = 30.0f32;
    for f in 0..frames {
        let t = span * f as f32 / frames as f32;
        let img = frame(&belt, w, h, &cam, t);
        enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(80, 1)))?;
    }
    Ok(())
}

/// A GIF that drifts the camera across the belt — the interactive "look around
/// the ring" feature captured as a tour. The camera eases out toward the rim and
/// back while the belt keeps revolving.
fn write_pan_gif(path: &str, seed: u32, w: u32, h: u32, frames: usize) -> Result<(), Box<dyn std::error::Error>> {
    let belt = Belt::generate(seed);
    let ext = belt.extent();
    let zoom = (w as f32 * 0.5 / (ext * 0.6)).clamp(0.7, 1.8);
    let file = File::create(path)?;
    let mut enc = GifEncoder::new(file);
    enc.set_repeat(Repeat::Infinite)?;
    for f in 0..frames {
        let u = f as f32 / frames as f32;
        // Ease the camera out to the rim and back (a there-and-back pan loops).
        let s = 0.5 - 0.5 * (u * std::f32::consts::TAU).cos(); // 0→1→0
        let cam = Camera { x: (ext * 0.55) * s, y: (-ext * 0.16) * s, zoom };
        let t = 20.0 * u;
        let img = frame(&belt, w, h, &cam, t);
        enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(80, 1)))?;
    }
    Ok(())
}

/// A framed poster still of the whole belt.
fn write_poster(path: &str, seed: u32, w: u32, h: u32, t: f32) -> Result<(), Box<dyn std::error::Error>> {
    let belt = Belt::generate(seed);
    let cam = Camera { x: 0.0, y: 0.0, zoom: fit_zoom(&belt, w, h) };
    frame(&belt, w, h, &cam, t).save(path)?;
    // Report what the seed produced.
    println!(
        "  seed {seed}: {} rocks, ring {:.0}..{:.0}",
        belt.rock_count(),
        belt.inner,
        belt.outer
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("out")?;

    // 1) the headline: the belt revolving under a fitted camera.
    write_orbit_gif("out/asteroid.gif", 7, 480, 300, 48)?;
    println!("Wrote out/asteroid.gif");

    // 2) the drift tour: pan the camera across the ring.
    write_pan_gif("out/asteroid_pan.gif", 7, 480, 300, 60)?;
    println!("Wrote out/asteroid_pan.gif");

    // 3) a handful of poster stills across seeds, to show the variety.
    println!("Posters:");
    for (i, seed) in [3u32, 7, 21, 42].iter().enumerate() {
        write_poster(&format!("out/asteroid_{}.png", i), *seed, 900, 520, 5.0)?;
    }

    Ok(())
}
