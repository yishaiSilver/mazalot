//! Performance profiler for the solar-system renderer.
//!
//! Decomposes `render_system`'s per-frame cost by rendering the SAME scene under
//! controlled scenarios and diffing the timings:
//!   • background is camera-independent in cost, so rendering with all bodies
//!     pushed off-screen (culled) isolates the background;
//!   • toggling `star_density` to 0 isolates the star pass;
//!   • zooming in past the nebula-fade isolates the base+orbit fill;
//!   • zooming onto the sun vs a planet compares the two body shaders.
//! Native only (uses `std::time::Instant`); run: `cargo run --release --bin bench`.

use solar::{render_system, render_system_cached, Camera, System};
use std::time::Instant;

const W: u32 = 1000;
const H: u32 = 640;
const FRAMES: u32 = 80;

/// Mean ms/frame for `render_system` over `FRAMES` (after a short warm-up),
/// with the camera PANNING (`bgx` advances) so it reflects the interactive drag
/// cost — the full-frame cache never hits, and the nebula cache re-bakes only
/// when the quantized scroll offset ticks (a realistic ~3 px/frame drag).
fn ms(sys: &System, cam: &Camera, buf: &mut [u8]) -> f64 {
    for i in 0..4 {
        render_system(sys, W, H, cam, i as f32 * 3.0, 0.0, i as f32 * 0.1, i as f32 * 0.1, i as f32 * 0.1, buf);
    }
    let t = Instant::now();
    for i in 0..FRAMES {
        let ta = i as f32 * 0.013; // advance clocks so nothing is cached away
        let bg = 12.0 + i as f32 * 3.0; // pan -> full-frame cache miss every frame
        render_system(sys, W, H, cam, bg, 0.0, ta, ta * 1.3, ta * 0.7, buf);
    }
    t.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64
}

fn set_density(sys: &mut System, density: f32) {
    sys.set_view(1.0, 1.0, 1.0, 1.0, 1.0, 160.0, 110.0, density, 1.0);
}

fn fit_zoom(sys: &System) -> f32 {
    let ext = sys.extent();
    (0.45 * W as f32 / ext).min(0.45 * H as f32 / (ext * 0.55))
}

fn main() {
    let mut buf = vec![0u8; (W * H * 4) as usize];
    let seed = 7u32;
    let mut sys = System::generate(seed);
    let fz = fit_zoom(&sys);
    println!("solar renderer profile — {W}x{H}, {FRAMES} frames/scenario, seed {seed}");
    println!("system: {} planets, fit zoom {:.3}\n", sys.planets.len(), fz);

    // Cameras.
    let fit = Camera { x: 0.0, y: 0.0, zoom: fz };
    let far = Camera { x: 1.0e7, y: 1.0e7, zoom: fz }; // all bodies off-screen -> background only
    let far_zoomed = Camera { x: 1.0e7, y: 1.0e7, zoom: fz * 12.0 }; // + nebula & far-layer faded
    let sun = Camera { x: 0.0, y: 0.0, zoom: fz * 22.0 }; // fill screen with the sun
    let planet = &sys.planets.iter().find(|p| p.orbit > 0.0).cloned();

    set_density(&mut sys, 0.5); // default
    let t_full = ms(&sys, &fit, &mut buf);
    let t_bg = ms(&sys, &far, &mut buf); // background, default density
    set_density(&mut sys, 0.0);
    let t_bg_nostars = ms(&sys, &far, &mut buf); // base + nebula + orbits
    let t_base = ms(&sys, &far_zoomed, &mut buf); // base fill + orbits (nebula faded)
    set_density(&mut sys, 0.5);
    let t_sun = ms(&sys, &sun, &mut buf); // sun tile dominates

    // A planet filling the view: follow it to centre, zoom in.
    let t_planet = if let Some(p) = planet {
        // place it at the top of its orbit (front) by choosing t so sin≈1
        let cam = Camera { x: p.orbit, y: 0.0, zoom: fz * 22.0 };
        ms(&sys, &cam, &mut buf)
    } else {
        0.0
    };

    let bodies = (t_full - t_bg).max(0.0);
    let stars = (t_bg - t_bg_nostars).max(0.0);
    let nebula = (t_bg_nostars - t_base).max(0.0);

    // Cached path on a STILL camera (the default "watch it orbit" view): the
    // backdrop is a memcpy, only the bodies re-render.
    let t_cached = {
        for i in 0..4 {
            render_system_cached(&mut sys, W, H, &fit, 0.0, 0.0, i as f32, i as f32, i as f32, &mut buf);
        }
        let t = Instant::now();
        for i in 0..FRAMES {
            let ta = i as f32 * 0.013;
            render_system_cached(&mut sys, W, H, &fit, 0.0, 0.0, ta, ta * 1.3, ta * 0.7, &mut buf);
        }
        t.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64
    };

    println!("── whole frame ──────────────────────────────");
    println!("  fit, panning (drag)               {:7.2} ms   ({:.0} fps)", t_full, 1000.0 / t_full);
    println!("  fit, still camera (cached bg)     {:7.2} ms   ({:.0} fps)  <- render_system_cached", t_cached, 1000.0 / t_cached);
    println!("  zoomed onto the sun               {:7.2} ms   ({:.0} fps)", t_sun, 1000.0 / t_sun);
    println!("  zoomed onto a planet              {:7.2} ms   ({:.0} fps)", t_planet, 1000.0 / t_planet);
    println!("\n── fit breakdown (panning) ──────────────────");
    println!("  background total                  {:7.2} ms   {:5.1}%", t_bg, 100.0 * t_bg / t_full);
    println!("    ├ base fill + orbit paths       {:7.2} ms   {:5.1}%", t_base, 100.0 * t_base / t_full);
    println!("    ├ nebula (bake amortized)       {:7.2} ms   {:5.1}%", nebula, 100.0 * nebula / t_full);
    println!("    └ stars (density 0.5)           {:7.2} ms   {:5.1}%", stars, 100.0 * stars / t_full);
    println!("  bodies (sun + {} planets)          {:7.2} ms   {:5.1}%", sys.planets.len(), bodies, 100.0 * bodies / t_full);

    // Zoomed OUT: the whole system shrinks to a few px, bodies get culled or
    // render into tiny (cheap, cached) tiles, so the BACKGROUND is ~all the cost.
    // And every layer is active — the far star layer + nebula fade IN at low
    // zoom (opposite of zooming onto a body), so the star pass is at its most
    // expensive here.
    println!("\n── zoomed out (zoom = fit x0.30) ────────────");
    let zout = Camera { x: 0.0, y: 0.0, zoom: fz * 0.30 };
    let zfar = Camera { x: 1.0e7, y: 1.0e7, zoom: fz * 0.30 }; // bodies off-screen
    set_density(&mut sys, 0.5);
    let z_full = ms(&sys, &zout, &mut buf);
    let z_bg = ms(&sys, &zfar, &mut buf); // background only, uncached (panning)
    set_density(&mut sys, 0.0);
    let z_nostars = ms(&sys, &zfar, &mut buf);
    set_density(&mut sys, 0.5);
    let z_still = {
        for i in 0..4 {
            render_system_cached(&mut sys, W, H, &zout, 0.0, 0.0, i as f32, i as f32, i as f32, &mut buf);
        }
        let t = Instant::now();
        for i in 0..FRAMES {
            let ta = i as f32 * 0.013;
            render_system_cached(&mut sys, W, H, &zout, 0.0, 0.0, ta, ta * 1.3, ta * 0.7, &mut buf);
        }
        t.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64
    };
    let z_bodies = (z_full - z_bg).max(0.0);
    let z_stars = (z_bg - z_nostars).max(0.0);
    println!("  panning (drag)                    {:7.2} ms   ({:.0} fps)", z_full, 1000.0 / z_full);
    println!("  still camera (cached bg)          {:7.2} ms   ({:.0} fps)", z_still, 1000.0 / z_still);
    println!("    ├ background, uncached          {:7.2} ms   {:5.1}%", z_bg, 100.0 * z_bg / z_full);
    println!("    │   └ stars (all 3 layers on)   {:7.2} ms   (fit: {:.2} ms)", z_stars, stars);
    println!("    └ bodies (tiny / culled)        {:7.2} ms   {:5.1}%", z_bodies, 100.0 * z_bodies / z_full);

    // Nebula cache: the per-cell fBm bake vs reusing it. On a still/zooming
    // camera the offset never ticks (nebula cached); on a slow drag it ticks
    // rarely; only a fast fling re-bakes most frames.
    println!("\n── nebula cache (bg only, density 0) ────────");
    set_density(&mut sys, 0.0);
    let bgcam = Camera { x: 1.0e7, y: 1.0e7, zoom: fz };
    let bake_ms = |sys: &System, step: f32, buf: &mut [u8]| -> f64 {
        for i in 0..4 {
            render_system(sys, W, H, &bgcam, i as f32 * step, 0.0, 0.0, 0.0, 0.0, buf);
        }
        let t = Instant::now();
        for i in 0..FRAMES {
            render_system(sys, W, H, &bgcam, 40.0 + i as f32 * step, 0.0, 0.0, 0.0, 0.0, buf);
        }
        t.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64
    };
    let neb_still = bake_ms(&sys, 0.0, &mut buf); // never ticks -> cache always hit
    let neb_drag = bake_ms(&sys, 3.0, &mut buf); // ~3 px/frame -> ticks ~1/7 frames
    let neb_fling = bake_ms(&sys, 5000.0, &mut buf); // re-bakes every frame (old cost)
    println!("  still / zooming (cache hit)       {:7.2} ms", neb_still);
    println!("  slow drag (~1 bake / 7 frames)    {:7.2} ms", neb_drag);
    println!("  re-bake every frame (was: always) {:7.2} ms", neb_fling);
    println!("  => fBm bake saved when cached      {:6.2} ms/frame", (neb_fling - neb_still).max(0.0));
    set_density(&mut sys, 0.5);

    // Resolution scaling (background is O(pixels)).
    println!("\n── background vs resolution (bodies off) ────");
    for &(rw, rh) in &[(500u32, 320u32), (1000, 640), (1600, 1000), (2000, 1280)] {
        let mut b = vec![0u8; (rw * rh * 4) as usize];
        let cam = Camera { x: 1.0e7, y: 1.0e7, zoom: fz };
        for i in 0..4 {
            render_system(&sys, rw, rh, &cam, 0.0, 0.0, i as f32, i as f32, i as f32, &mut b);
        }
        let t = Instant::now();
        for i in 0..FRAMES {
            render_system(&sys, rw, rh, &cam, 0.0, 0.0, i as f32 * 0.01, 0.0, 0.0, &mut b);
        }
        let m = t.elapsed().as_secs_f64() * 1000.0 / FRAMES as f64;
        println!("  {:>4}x{:<4} ({:>5.0}k px)             {:7.2} ms   {:5.2} ns/px", rw, rh, (rw * rh) as f64 / 1000.0, m, m * 1.0e6 / (rw * rh) as f64);
    }

    // Star density scaling.
    println!("\n── star-pass cost vs density (bg only) ──────");
    for &den in &[0.0f32, 0.5, 1.0, 2.0] {
        set_density(&mut sys, den);
        let m = ms(&sys, &far, &mut buf);
        println!("  density {:.1}                       {:7.2} ms", den, m);
    }
    set_density(&mut sys, 0.5);

    // Per-frame heap allocations in the hot path (informational).
    println!("\nnote: the nebula field (~{} KB) is now cached on the System and reused",
        ((W + 7) / 8) * ((H + 7) / 8) * 12 / 1024);
    println!("      across frames; render_system still allocates a small draw-order Vec.");
}
