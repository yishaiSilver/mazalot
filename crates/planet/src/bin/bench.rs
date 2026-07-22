//! Feature-cost benchmark for the current renderer.
use planet::{param, render_rgba_styled, type_count, type_name, NUM_PARAMS};
use std::time::Instant;

fn idx(n: &str) -> usize {
    (0..type_count()).find(|&i| type_name(i) == n).unwrap()
}
fn defs(t: usize) -> Vec<f32> {
    (0..NUM_PARAMS).map(|i| param(t, i as u32)).collect()
}

fn bench(size: u32, t: usize, pal: u32, dith: f32, moons: u32, iters: u32) -> f64 {
    let p = defs(t);
    let mut buf = vec![0u8; (size * size * 4) as usize];
    for i in 0..8 {
        render_rgba_styled(size, t, 1, i as f32 * 0.1, &p, pal, dith, moons, &mut buf);
    }
    let s = Instant::now();
    for i in 0..iters {
        render_rgba_styled(size, t, 1, i as f32 * 0.01, &p, pal, dith, moons, &mut buf);
    }
    s.elapsed().as_nanos() as f64 / iters as f64
}

fn blit(size: u32, iters: u32) -> f64 {
    let n = (size * size * 4) as usize;
    let src = vec![7u8; n];
    let mut dst = vec![0u8; n];
    let s = Instant::now();
    for _ in 0..iters {
        dst.copy_from_slice(&src);
        std::hint::black_box(&dst);
    }
    s.elapsed().as_nanos() as f64 / iters as f64
}

fn main() {
    let it = 400;
    for &size in &[64u32, 128] {
        println!("\n=== {size}x{size} ===");
        let b = blit(size, it * 20);
        let row = |name: &str, ns: f64| {
            println!("{:<34} {:>7.3} ms  ~{:>5.0} fps  {:>5.0}x blit", name, ns / 1e6, 1e9 / ns, ns / b);
        };
        row("sprite blit (memcpy)", b);
        row("iron  (no weather)", bench(size, idx("iron"), 0, 0.7, 0, it));
        row("terran (clouds+aurora+storm+shimmer)", bench(size, idx("terran"), 0, 0.7, 0, it));
        row("gas_giant (warp bands+spot+aurora)", bench(size, idx("gas_giant"), 0, 0.7, 0, it));
        row("lava (cycling glow)", bench(size, idx("lava"), 0, 0.7, 0, it));
        row("barren (worley craters)", bench(size, idx("barren"), 0, 0.7, 0, it));
        println!("  -- feature deltas (on terran) --");
        let base = bench(size, idx("terran"), 0, 0.0, 0, it);
        row("terran, no dither/moons (baseline)", base);
        let d = bench(size, idx("terran"), 0, 0.7, 0, it);
        println!("    + dither                       +{:>6.3} ms", (d - base) / 1e6);
        let m = bench(size, idx("terran"), 0, 0.0, 1, it);
        println!("    + moons                        +{:>6.3} ms", (m - base) / 1e6);
        let pl = bench(size, idx("terran"), 1, 0.0, 0, it);
        println!("    + palette (game boy)           +{:>6.3} ms", (pl - base) / 1e6);
    }
}
