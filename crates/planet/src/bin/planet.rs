//! Native planet generator: turns `planet-core`'s RGBA frames into GIFs, a
//! contact-sheet PNG, and a combined all-types GIF.
//!
//! All planet math + the 26-type table live in the `planet-core` crate (shared
//! with the web/WASM build). This file is only the `image`-crate orchestration,
//! now delegated to the shared `render-io` helpers.

use planet::{render_rgba, type_count, type_name};
use render_io::{parallel_map, write_contact_sheet, write_spin_gif, write_spin_grid_gif, RgbaImage};

const NATIVE: u32 = 64; // render resolution (px)
const FRAMES: usize = 30; // frames per full rotation
const GIF_UP: u32 = 3; // nearest-neighbour zoom for individual GIFs
const POSTER_UP: u32 = 2; // zoom for the contact-sheet PNG

/// One native-resolution frame via the shared core.
fn render_frame(type_idx: usize, seed: u32, angle: f32) -> RgbaImage {
    let mut buf = vec![0u8; (NATIVE * NATIVE * 4) as usize];
    render_rgba(NATIVE, type_idx, seed, angle, &mut buf);
    RgbaImage::from_raw(NATIVE, NATIVE, buf).expect("buffer size matches")
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("out")?;
    let count = type_count();

    // 1) one spinning GIF per type. Fanned out a whole GIF at a time: that puts
    // each type's GIF *encode* — quantization, not sprite math, and by far the
    // longest serial stretch here — on the pool alongside the renders. The
    // inner per-frame fan-out inside `write_spin_gif` collapses to a serial
    // loop under it, which is what we want with 26 jobs already in flight.
    let written = parallel_map(count, |i| -> Result<String, String> {
        let path = format!("out/planet_{}.gif", type_name(i));
        write_spin_gif(&path, FRAMES, 70, GIF_UP, move |angle| {
            render_frame(i, 100 + i as u32 * 13, angle)
        })
        // `Box<dyn Error>` is not `Send`, so it cannot cross a job boundary.
        .map_err(|e| e.to_string())?;
        Ok(path)
    });
    // Reported in type order, exactly as the serial loop did.
    for w in written {
        println!("Wrote {}", w?);
    }

    // 1b) all types spinning together
    write_spin_grid_gif("out/planets_all.gif", count, 6, FRAMES, 70, 2, [6, 6, 14, 255], 2, |i, angle| {
        render_frame(i, 100 + i as u32 * 13, angle)
    })?;
    println!("Wrote out/planets_all.gif");

    // 2) aggregate table PNG: one row per type, several seeds across
    let cols = 6u32;
    write_contact_sheet("out/planets_table.png", count as u32, cols, 3, [6, 6, 14, 255], POSTER_UP, |r, c| {
        let seed = r * 100 + c * 7 + 1;
        let angle = 0.5 + c as f32 * 0.32;
        render_frame(r as usize, seed, angle)
    })?;
    println!("Wrote out/planets_table.png ({} types x {} seeds = {} planets)", count, cols, count as u32 * cols);
    Ok(())
}
