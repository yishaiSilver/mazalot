//! Native planet-with-moons generator: turns the `moon` crate's RGBA scenes into
//! an orbiting GIF (moons circling — and passing in front of and behind — the
//! parent) plus a few framed poster PNGs across seeds.
//!
//! All scene math + the type tables live in the `moon` crate (shared with the
//! web/WASM build). This file is only the orchestration, now handed to the
//! shared `render-io` GIF/poster writers — the same spirit as solar's native
//! bin, self-contained in `moon`.

use moon::{moon_kind_name, parent_kind_name, Camera, MoonSystem};
use render_io::fit_zoom;

/// A GIF where the moons orbit under a fixed, fitted camera.
fn write_orbit_gif(path: &str, seed: u32, w: u32, h: u32, frames: usize) -> Result<(), Box<dyn std::error::Error>> {
    let sys = MoonSystem::generate(seed);
    // Orbits are squashed vertically (0.6), so the horizontal span is tighter.
    let cam = Camera { x: 0.0, y: 0.0, zoom: fit_zoom(sys.extent(), w, h, 0.92, 0.6) };
    // Sweep enough time that the inner moons make a lap or two.
    let span = 28.0f32;
    render_io::write_orbit_gif(path, w, h, frames, span, 80, |w, h, t| {
        let mut buf = vec![0u8; (w * h * 4) as usize];
        sys.render(w, h, &cam, t, &mut buf);
        buf
    })
}

/// A framed poster still of the planet + moons.
fn write_poster(path: &str, seed: u32, w: u32, h: u32, t: f32) -> Result<(), Box<dyn std::error::Error>> {
    let sys = MoonSystem::generate(seed);
    let cam = Camera { x: 0.0, y: 0.0, zoom: fit_zoom(sys.extent(), w, h, 0.92, 0.6) };
    render_io::write_poster(path, w, h, t, |w, h, t| {
        let mut buf = vec![0u8; (w * h * 4) as usize];
        sys.render(w, h, &cam, t, &mut buf);
        buf
    })?;
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
