//! Native solar-system generator: turns the `solar` crate's RGBA scenes into an
//! orbiting GIF, a camera-pan "drag tour" GIF, and framed poster PNGs.
//!
//! All system math + the type tables live in the `solar` crate (shared with the
//! web/WASM build). This file is only the `image`-crate orchestration — now via
//! the shared `render-io` helpers — the same spirit as planet's/star's native
//! bins, self-contained in `solar`.

use solar::{planet_kind_name, render_system, sun_kind_name, Camera, System};

/// A GIF where the planets orbit under a fixed, fitted camera.
fn write_orbit_gif(path: &str, seed: u32, w: u32, h: u32, frames: usize) -> Result<(), Box<dyn std::error::Error>> {
    let sys = System::generate(seed);
    // Orbits are squashed vertically (~0.42), so fit the smaller half-span with a
    // comfortable margin.
    let zoom = render_io::fit_zoom(sys.extent(), w, h, 0.92, 0.55);
    // Sweep enough time that the inner planets make a lap or two.
    let span = 26.0f32;
    render_io::write_orbit_gif(path, w, h, frames, span, 80, |w, h, t| {
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let cam = Camera { x: 0.0, y: 0.0, zoom };
        // Native uses one clock for orbit / spin / sun alike; the background scroll
        // is the camera pan in screen space (cam·zoom), correct at fixed zoom.
        render_system(&sys, w, h, &cam, cam.x * cam.zoom, cam.y * cam.zoom, t, t, t, &mut buf);
        buf
    })
}

/// A GIF that drags the camera across the system — the interactive "look around
/// at whatever's orbiting" feature, captured as a tour. The camera eases from
/// the outer edge back through the star while the planets keep orbiting.
fn write_pan_gif(path: &str, seed: u32, w: u32, h: u32, frames: usize) -> Result<(), Box<dyn std::error::Error>> {
    let sys = System::generate(seed);
    let ext = sys.extent();
    let zoom = (w as f32 * 0.5 / (ext * 0.55)).min(1.6).max(0.6);
    render_io::write_anim_gif(path, w, h, frames, 80, |f, frames, w, h| {
        let u = f as f32 / frames as f32;
        // Ease the camera out to the rim and back (a there-and-back pan loops).
        let s = 0.5 - 0.5 * (u * std::f32::consts::TAU).cos(); // 0→1→0
        let cam = Camera { x: (ext * 0.62) * s, y: (-ext * 0.14) * s, zoom };
        let t = 18.0 * u;
        let mut buf = vec![0u8; (w * h * 4) as usize];
        render_system(&sys, w, h, &cam, cam.x * cam.zoom, cam.y * cam.zoom, t, t, t, &mut buf);
        buf
    })
}

/// A framed poster still of the whole system.
fn write_poster(path: &str, seed: u32, w: u32, h: u32, t: f32) -> Result<(), Box<dyn std::error::Error>> {
    let sys = System::generate(seed);
    let zoom = render_io::fit_zoom(sys.extent(), w, h, 0.92, 0.55);
    render_io::write_poster(path, w, h, t, |w, h, t| {
        let mut buf = vec![0u8; (w * h * 4) as usize];
        let cam = Camera { x: 0.0, y: 0.0, zoom };
        render_system(&sys, w, h, &cam, cam.x * cam.zoom, cam.y * cam.zoom, t, t, t, &mut buf);
        buf
    })?;
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
