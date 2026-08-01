# noise-core

The bottom of everything: 3D value-noise, fBm, domain warp, Worley, and the
colour/ramp math every other crate shades with. No third-party dependencies.

## What it provides

- **`hash3` / `value_noise`** — the lattice. Integer `u32` math: wrapping
  multiplies, xors, shifts, no transcendentals.
- **fBm stack** — `fbm`, `fbm_warp`, and `fbm_warp_oct`, which takes an explicit
  octave budget so a caller can drop octaves the pixel grid cannot resolve.
- **Worley** — cellular noise, used for granulation and cratered worlds.
- **Colour** — ramp evaluation, mixing, and the palette helpers.

A domain warp's *displacement* field only ever needs two octaves, whatever the
size (`fbm_warp_oct`). That alone turns a 4-octave `fbm_warp`'s 16 octave
evaluations into 10.

## simd128 is load-bearing, not a bonus

The two lattice kernels are written four-lane (`lanes.rs`) and hash four corners
per instruction on wasm `simd128`. **With** the feature they beat the old scalar
code; **without** it, the portable array fallback is *slower* in wasm than what it
replaced. Never build a demo module with it off.

`simd128` comes from `.cargo/config.toml`, which cargo reads from the *working
directory* — so wasm builds must run from the repo root. A scratch crate built
outside the repo silently loses it and will look like a regression.

Do not reach for `relaxed-simd` to go further: it permits FMA, whose rounding
would split wasm output from the native generators'. `lanes.rs` documents the
rules that keep the two backends bit-identical — read it before adding an
operation there.

`value_noise` runs ~28× per pixel and is `#[inline(never)]` **on the vector path
only**: inlined, its `v128` temporaries spill in the pixel loop and it comes out
slower than scalar. That reversed sign between a microbenchmark and a real frame,
so measure whole frames.

## It transliterates to GLSL exactly

`src/noise.glsl` is the prelude every other GLSL port is concatenated after: it
carries `#version`, the lattice kernels, and (riding along) `dither-core`'s
`dither.glsl`.

> Because `hash3` and `value_noise` are integer math, there is nothing a driver
> is free to round its own way. They transliterate into GLSL ES 3.00 *exactly*,
> so the lattice under the GPU picture is bit-identical to the lattice under the
> CPU one. Worley and the fBm stack fall out of that for free.

That is why porting the renderers to WebGL2 was ~200 lines of ramps and mixes
rather than a rewrite. See [planet-core](../planet-core/README.md#webgl2) and
[background-core](../background-core/README.md).

## Verifying a change

Touching this crate moves every image in the workspace. Hash `out/` before and
after (see the [root README](../../README.md#verifying-a-change)), and check the
*wasm* path too — instantiate a module in node, render, hash the pixels. `out/`
only covers the native build.
