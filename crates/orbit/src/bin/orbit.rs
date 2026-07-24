//! Native orbit generator: turns the `orbit` crate's RGBA scenes into a GIF of
//! bodies tracing their eccentric, inclined Keplerian orbits (clearly racing
//! through perihelion and crawling at aphelion), plus a couple of poster PNGs
//! across seeds to show the eccentricity + inclination variety.
//!
//! All the orbital math + type tables live in the `orbit` crate (shared with the
//! web/WASM build). This file is only the `image`-crate orchestration — the same
//! spirit as solar's/star's native bins, self-contained in `orbit`.

use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame, RgbaImage};
use orbit::{body_kind_name, sun_kind_name, OrbitSystem};
use std::fs::File;

/// Render one scene frame straight into an `RgbaImage` (auto zoom-to-fit camera).
fn frame(sys: &OrbitSystem, w: u32, h: u32, t: f32) -> RgbaImage {
    let mut buf = vec![0u8; (w * h * 4) as usize];
    sys.render(w, h, None, t, &mut buf);
    RgbaImage::from_raw(w, h, buf).expect("buffer size matches")
}

/// A GIF where the bodies orbit under a fixed, fitted camera. Because position
/// comes from the Kepler solve, each body visibly speeds up at perihelion and
/// slows at aphelion.
fn write_orbit_gif(path: &str, seed: u32, w: u32, h: u32, frames: usize) -> Result<(), Box<dyn std::error::Error>> {
    let sys = OrbitSystem::generate(seed);
    let file = File::create(path)?;
    let mut enc = GifEncoder::new(file);
    enc.set_repeat(Repeat::Infinite)?;
    // Sweep enough time that the inner bodies make a lap or two.
    let span = 30.0f32;
    for f in 0..frames {
        let t = span * f as f32 / frames as f32;
        let img = frame(&sys, w, h, t);
        enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(80, 1)))?;
    }
    Ok(())
}

/// A framed poster still of the whole system.
fn write_poster(path: &str, seed: u32, w: u32, h: u32, t: f32) -> Result<(), Box<dyn std::error::Error>> {
    let sys = OrbitSystem::generate(seed);
    frame(&sys, w, h, t).save(path)?;
    // Report what the seed produced, with each body's eccentricity.
    print!("  seed {seed}: {} star +", sun_kind_name(sys.sun_kind));
    for (i, _) in (0..sys.body_count()).enumerate() {
        print!(" {}(e={:.2})", body_kind_name(sys.body_kind(i)), sys.body_eccentricity(i));
    }
    println!();
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("out")?;

    // 1) the headline: bodies tracing eccentric inclined orbits, fast at
    //    perihelion, slow at aphelion.
    write_orbit_gif("out/orbit.gif", 7, 480, 360, 60)?;
    println!("Wrote out/orbit.gif");

    // 2) a second GIF on another seed for a different ellipse/tilt mix.
    write_orbit_gif("out/orbit_b.gif", 21, 480, 360, 60)?;
    println!("Wrote out/orbit_b.gif");

    // 3) poster stills across seeds, to show the variety.
    println!("Posters:");
    for (i, seed) in [3u32, 7, 21, 42].iter().enumerate() {
        write_poster(&format!("out/orbit_{}.png", i), *seed, 900, 600, 5.0)?;
    }

    Ok(())
}
