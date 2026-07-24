//! Native planet-with-moons generator: turns the `moon` crate's RGBA scenes into
//! an orbiting GIF (moons circling — and passing in front of and behind — the
//! parent) plus a few framed poster PNGs across seeds.
//!
//! All scene math + the type tables live in the `moon` crate (shared with the
//! web/WASM build). This file is only the `image`-crate orchestration — the same
//! spirit as solar's native bin, self-contained in `moon`.

use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame, RgbaImage};
use moon::{moon_kind_name, parent_kind_name, Camera, MoonSystem};
use std::fs::File;

/// Render one scene frame straight into an `RgbaImage`.
fn frame(sys: &MoonSystem, w: u32, h: u32, cam: &Camera, t: f32) -> RgbaImage {
    let mut buf = vec![0u8; (w * h * 4) as usize];
    sys.render(w, h, cam, t, &mut buf);
    RgbaImage::from_raw(w, h, buf).expect("buffer size matches")
}

/// Zoom that fits the whole system into a `w`x`h` viewport with margin. Orbits
/// are squashed vertically, so the horizontal span is the tighter axis.
fn fit_zoom(sys: &MoonSystem, w: u32, h: u32) -> f32 {
    let ext = sys.extent();
    let halfw = w as f32 * 0.5 * 0.92;
    let halfh = h as f32 * 0.5 * 0.92;
    (halfw / ext).min(halfh / (ext * 0.6))
}

/// A GIF where the moons orbit under a fixed, fitted camera.
fn write_orbit_gif(path: &str, seed: u32, w: u32, h: u32, frames: usize) -> Result<(), Box<dyn std::error::Error>> {
    let sys = MoonSystem::generate(seed);
    let cam = Camera { x: 0.0, y: 0.0, zoom: fit_zoom(&sys, w, h) };
    let file = File::create(path)?;
    let mut enc = GifEncoder::new(file);
    enc.set_repeat(Repeat::Infinite)?;
    // Sweep enough time that the inner moons make a lap or two.
    let span = 28.0f32;
    for f in 0..frames {
        let t = span * f as f32 / frames as f32;
        let img = frame(&sys, w, h, &cam, t);
        enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(80, 1)))?;
    }
    Ok(())
}

/// A framed poster still of the planet + moons.
fn write_poster(path: &str, seed: u32, w: u32, h: u32, t: f32) -> Result<(), Box<dyn std::error::Error>> {
    let sys = MoonSystem::generate(seed);
    let cam = Camera { x: 0.0, y: 0.0, zoom: fit_zoom(&sys, w, h) };
    frame(&sys, w, h, &cam, t).save(path)?;
    // Report what the seed produced.
    print!("  seed {seed}: {} +", parent_kind_name(sys.parent_kind));
    for m in &sys.moons {
        print!(" {}", moon_kind_name(m.kind));
    }
    println!();
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("out")?;

    // 1) the headline: moons orbiting a fitted parent planet.
    write_orbit_gif("out/moon.gif", 7, 420, 300, 56)?;
    println!("Wrote out/moon.gif");

    // 2) a handful of poster stills across seeds, to show the variety.
    println!("Posters:");
    for (i, seed) in [3u32, 7, 21, 42].iter().enumerate() {
        write_poster(&format!("out/moon_{}.png", i), *seed, 720, 480, 5.0)?;
    }

    Ok(())
}
