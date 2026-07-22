//! Native solar-system generator: turns the `solar` crate's RGBA scenes into an
//! orbiting GIF, a camera-pan "drag tour" GIF, and framed poster PNGs.
//!
//! All system math + the type tables live in the `solar` crate (shared with the
//! web/WASM build). This file is only the `image`-crate orchestration — the same
//! spirit as planet's/star's native bins, self-contained in `solar`.

use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame, RgbaImage};
use solar::{planet_kind_name, render_system, sun_kind_name, Camera, System};
use std::fs::File;

/// Render one scene frame straight into an `RgbaImage`.
fn frame(sys: &System, w: u32, h: u32, cam: &Camera, t: f32) -> RgbaImage {
    let mut buf = vec![0u8; (w * h * 4) as usize];
    render_system(sys, w, h, cam, t, &mut buf);
    RgbaImage::from_raw(w, h, buf).expect("buffer size matches")
}

/// Zoom that fits the whole system into a `w`x`h` viewport with margin.
fn fit_zoom(sys: &System, w: u32, h: u32) -> f32 {
    let ext = sys.extent();
    // Orbits are squashed vertically (~0.42), so height is the tighter axis only
    // by a little; fit against the smaller half-span with a comfortable margin.
    let halfw = w as f32 * 0.5 * 0.92;
    let halfh = h as f32 * 0.5 * 0.92;
    (halfw / ext).min(halfh / (ext * 0.55))
}

/// A GIF where the planets orbit under a fixed, fitted camera.
fn write_orbit_gif(path: &str, seed: u32, w: u32, h: u32, frames: usize) -> Result<(), Box<dyn std::error::Error>> {
    let sys = System::generate(seed);
    let cam = Camera { x: 0.0, y: 0.0, zoom: fit_zoom(&sys, w, h) };
    let file = File::create(path)?;
    let mut enc = GifEncoder::new(file);
    enc.set_repeat(Repeat::Infinite)?;
    // Sweep enough time that the inner planets make a lap or two.
    let span = 26.0f32;
    for f in 0..frames {
        let t = span * f as f32 / frames as f32;
        let img = frame(&sys, w, h, &cam, t);
        enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(80, 1)))?;
    }
    Ok(())
}

/// A GIF that drags the camera across the system — the interactive "look around
/// at whatever's orbiting" feature, captured as a tour. The camera eases from
/// the outer edge back through the star while the planets keep orbiting.
fn write_pan_gif(path: &str, seed: u32, w: u32, h: u32, frames: usize) -> Result<(), Box<dyn std::error::Error>> {
    let sys = System::generate(seed);
    let ext = sys.extent();
    let zoom = (w as f32 * 0.5 / (ext * 0.55)).min(1.6).max(0.6);
    let file = File::create(path)?;
    let mut enc = GifEncoder::new(file);
    enc.set_repeat(Repeat::Infinite)?;
    for f in 0..frames {
        let u = f as f32 / frames as f32;
        // Ease the camera out to the rim and back (a there-and-back pan loops).
        let s = 0.5 - 0.5 * (u * std::f32::consts::TAU).cos(); // 0→1→0
        let cam = Camera { x: (ext * 0.62) * s, y: (-ext * 0.14) * s, zoom };
        let t = 18.0 * u;
        let img = frame(&sys, w, h, &cam, t);
        enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(80, 1)))?;
    }
    Ok(())
}

/// A framed poster still of the whole system.
fn write_poster(path: &str, seed: u32, w: u32, h: u32, t: f32) -> Result<(), Box<dyn std::error::Error>> {
    let sys = System::generate(seed);
    let cam = Camera { x: 0.0, y: 0.0, zoom: fit_zoom(&sys, w, h) };
    frame(&sys, w, h, &cam, t).save(path)?;
    // Report what the seed produced.
    print!("  seed {seed}: {} +", sun_kind_name(sys.sun_kind));
    for p in &sys.planets {
        print!(" {}", planet_kind_name(p.kind));
    }
    println!();
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("out")?;

    // 1) the headline: planets orbiting a fitted star.
    write_orbit_gif("out/solar.gif", 7, 480, 300, 48)?;
    println!("Wrote out/solar.gif");

    // 2) the drag tour: pan the camera across the system.
    write_pan_gif("out/solar_pan.gif", 7, 480, 300, 60)?;
    println!("Wrote out/solar_pan.gif");

    // 3) a handful of poster stills across seeds, to show the variety.
    println!("Posters:");
    for (i, seed) in [3u32, 7, 21, 42].iter().enumerate() {
        write_poster(&format!("out/solar_{}.png", i), *seed, 900, 520, 6.0)?;
    }

    Ok(())
}
