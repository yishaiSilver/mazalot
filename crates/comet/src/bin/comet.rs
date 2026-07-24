//! Native comet generator: turns the `comet` crate's RGBA scenes into an
//! orbiting GIF (the comet sweeping through perihelion, tail swinging to stay
//! anti-sunward) plus framed poster PNGs across seeds.
//!
//! All orbit + tail math lives in the `comet` crate (shared with the web/WASM
//! build). This file is only the `image`-crate orchestration — the same spirit
//! as `solar`'s native bin, self-contained in `comet`.

use comet::{star_kind_name, Camera, CometScene};
use image::codecs::gif::{GifEncoder, Repeat};
use image::{Delay, Frame, RgbaImage};
use std::fs::File;

/// Render one scene frame straight into an `RgbaImage`.
fn frame(scene: &CometScene, w: u32, h: u32, cam: &Camera, t: f32) -> RgbaImage {
    let mut buf = vec![0u8; (w * h * 4) as usize];
    scene.render(w, h, cam, t, &mut buf);
    RgbaImage::from_raw(w, h, buf).expect("buffer size matches")
}

/// Zoom that fits the whole orbit into a `w`x`h` viewport with margin. Orbits
/// are squashed vertically (~0.42), so height is the tighter axis by only a
/// little; fit against the smaller half-span with a comfortable margin.
fn fit_zoom(scene: &CometScene, w: u32, h: u32) -> f32 {
    let ext = scene.extent();
    let halfw = w as f32 * 0.5 * 0.9;
    let halfh = h as f32 * 0.5 * 0.9;
    (halfw / ext).min(halfh / (ext * 0.6))
}

/// A GIF of the comet(s) sweeping a full orbit under a fixed, fitted camera —
/// visibly accelerating through perihelion, tail always pointing away from the
/// star. The first comet's period sets the loop so it closes seamlessly.
fn write_orbit_gif(path: &str, seed: u32, w: u32, h: u32, frames: usize) -> Result<(), Box<dyn std::error::Error>> {
    let scene = CometScene::generate(seed);
    let cam = Camera { x: 0.0, y: 0.0, zoom: fit_zoom(&scene, w, h) };
    // One full period of the primary comet, so the animation loops perfectly.
    let span = scene.comets.first().map(|c| c.period).unwrap_or(12.0);
    let file = File::create(path)?;
    let mut enc = GifEncoder::new(file);
    enc.set_repeat(Repeat::Infinite)?;
    for f in 0..frames {
        let t = span * f as f32 / frames as f32;
        let img = frame(&scene, w, h, &cam, t);
        enc.encode_frame(Frame::from_parts(img, 0, 0, Delay::from_numer_denom_ms(70, 1)))?;
    }
    Ok(())
}

/// A framed poster still, timed near perihelion so the tail is at its longest.
fn write_poster(path: &str, seed: u32, w: u32, h: u32) -> Result<(), Box<dyn std::error::Error>> {
    let scene = CometScene::generate(seed);
    let cam = Camera { x: 0.0, y: 0.0, zoom: fit_zoom(&scene, w, h) };
    // Perihelion happens when mean anomaly ≡ 0, i.e. t·(2π/period) = −phase.
    let c = &scene.comets[0];
    let t_peri = (-c.phase / std::f32::consts::TAU) * c.period + c.period; // first peri >= 0
    frame(&scene, w, h, &cam, t_peri).save(path)?;
    // Report what the seed produced.
    println!(
        "  seed {seed}: {} + {} comet(s), e[0]={:.2}, peri/aph={:.0}/{:.0}",
        star_kind_name(scene.star_kind),
        scene.comets.len(),
        c.e,
        c.perihelion(),
        c.aphelion(),
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("out")?;

    // 1) the headline: a comet sweeping through perihelion and back out.
    write_orbit_gif("out/comet.gif", 7, 480, 300, 60)?;
    println!("Wrote out/comet.gif");

    // 2) a second seed with (likely) multiple comets, for variety.
    write_orbit_gif("out/comet_multi.gif", 3, 480, 300, 60)?;
    println!("Wrote out/comet_multi.gif");

    // 3) a handful of poster stills across seeds, timed at perihelion.
    println!("Posters:");
    for (i, seed) in [1u32, 7, 21, 42].iter().enumerate() {
        write_poster(&format!("out/comet_{}.png", i), *seed, 900, 520)?;
    }

    Ok(())
}
