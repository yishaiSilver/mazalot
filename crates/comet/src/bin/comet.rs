//! Native comet generator: turns the `comet` crate's RGBA scenes into an
//! orbiting GIF (the comet sweeping through perihelion, tail swinging to stay
//! anti-sunward) plus framed poster PNGs across seeds.
//!
//! All orbit + tail math lives in the `comet` crate (shared with the web/WASM
//! build). The `image`-crate orchestration (GIF/PNG encoding, fit-zoom) is the
//! shared `render-io` helper; this file only owns the per-frame render call and
//! the crate's `Camera`, which `render-io` never sees.

use comet::{star_kind_name, Camera, CometScene};

/// Zoom that fits the whole orbit into a `w`x`h` viewport with margin. Orbits
/// are squashed vertically (~0.42), so height is the tighter axis by only a
/// little; fit against the smaller half-span with a comfortable margin.
fn fit_zoom(scene: &CometScene, w: u32, h: u32) -> f32 {
    render_io::fit_zoom(scene.extent(), w, h, 0.9, 0.6)
}

/// A GIF of the comet(s) sweeping a full orbit under a fixed, fitted camera —
/// visibly accelerating through perihelion, tail always pointing away from the
/// star. The first comet's period sets the loop so it closes seamlessly.
fn write_orbit_gif(path: &str, seed: u32, w: u32, h: u32, frames: usize) -> Result<(), Box<dyn std::error::Error>> {
    let scene = CometScene::generate(seed);
    let cam = Camera { x: 0.0, y: 0.0, zoom: fit_zoom(&scene, w, h) };
    // One full period of the primary comet, so the animation loops perfectly.
    let span = scene.comets.first().map(|c| c.period).unwrap_or(12.0);
    render_io::write_orbit_gif(path, w, h, frames, span, 70, |w, h, t| {
        let mut buf = vec![0u8; (w * h * 4) as usize];
        scene.render(w, h, &cam, t, &mut buf);
        buf
    })
}

/// A framed poster still, timed near perihelion so the tail is at its longest.
fn write_poster(path: &str, seed: u32, w: u32, h: u32) -> Result<(), Box<dyn std::error::Error>> {
    let scene = CometScene::generate(seed);
    let cam = Camera { x: 0.0, y: 0.0, zoom: fit_zoom(&scene, w, h) };
    // Perihelion happens when mean anomaly ≡ 0, i.e. t·(2π/period) = −phase.
    let c = &scene.comets[0];
    let t_peri = (-c.phase / std::f32::consts::TAU) * c.period + c.period; // first peri >= 0
    render_io::write_poster(path, w, h, t_peri, |w, h, t| {
        let mut buf = vec![0u8; (w * h * 4) as usize];
        scene.render(w, h, &cam, t, &mut buf);
        buf
    })?;
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
