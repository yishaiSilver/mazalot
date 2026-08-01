# sun-core

The compact star tile — granulation and corona, cut out on transparency and sized
to its disc — used by [`solar`](../solar/README.md) and
[`comet`](../comet/README.md) as a scene body. The hero framing of a star lives in
[`star`](../star/README.md).

A star is the **inverse of a planet**: self-luminous, so there is no terminator and
no external light. The whole disc glows.

## The tile is cached, and the spike is what matters

The convection/corona shader (27-cell Worley + fBm per pixel over a large tile) was
the single worst body in a scene at ~39 ms. The boil evolves slowly, so `SunCache`
keys the tile on render radius, the clip rect, and a quantized boil clock
(`SUN_TQUANT`), and re-bakes every few frames instead of every frame: **39 → ~8 ms**.
A still or non-rotating star is essentially free. At extreme zoom the tile also
drops its secondary-fBm octaves, which are below the dither floor at that size.

Two things follow:

- **The clip is part of the key.** Pixels outside `visible_tile_rect` are never
  shaded, so a tile is only valid for the placement its clip came from — and the
  rect is snapped outward to a grid (`snap_out`), or a camera drifting a pixel a
  frame invalidates the cache every frame and the caching buys nothing.
- **Cost arrives as a spike.** Most frames are a blit and one in ~5 pays the whole
  re-bake, so what matters is the size of that one frame, not the average.
- A time step smaller than `SUN_TQUANT` means the shader never runs at all — worth
  knowing before benchmarking a scene and concluding the star is free.

## The corona is tabulated, not shaded

Two observations shrank the re-bake, and neither is the planets' fix — the star's
fBm fields are so low-frequency that their octave counts are already under Nyquist
at any scene radius.

**The corona is the majority of the tile.** The halo annulus out to
`1 + corona_reach` radii is ~1.9× the disc's area, so ~65% of shaded pixels are
corona (71.9k vs 38.0k at a 110 px radius) — and every one of them was running a
two-octave fBm and a `powf` to evaluate something that varies along *one* axis. The
streamers depend only on the angle around the limb, the falloff only on distance
past it, and the disc's limb darkening only on `mu`. So `Shade` samples each along
its own axis once per bake and interpolates.

Indexing the angular table needs a monotone angle, which `diamond_angle` gives for
a divide and a compare instead of an `atan2`. The table is sized to the halo's
outer circumference **×2.2**, because `diamond_angle` covers a turn in 4 units but
not at a constant rate — it is twice as steep at the diagonals. Shrink that factor
and you get angular stair-steps in the halo that no still frame makes obvious; the
tests pin it against direct evaluation.

**The old code recovered an angle with `atan2` only to feed it straight back
through `cos`/`sin`.** Out past the limb the unit direction is just `(nx, ny)/r` —
same field, three transcendentals cheaper, on those same 65% of pixels.

Sun alone, in the browser at 1680×944, cap 110: **22.7 → 15.8 ms** per frame at a
screen-filling zoom, **20.7 → 7.6 ms** when it overflows the viewport, with the
re-bake spike roughly 59 → 38 ms.

`cargo test -p sun-core` pins the tabulated fields against direct evaluation —
exact to the byte from a 24 px radius up.

## WebGL2

`src/star.glsl` ports the tile, concatenated after `noise-core`'s prelude.
`gl_uniforms()` transports the `SUNS` table rather than duplicating it, and the
`S_BLOTCH_OCT` / `S_CORONA_OCT` slots are an unpinned wire format — renumber one
and the GPU silently reads the wrong octave count.

There is no cache on the GPU path: no bake, so `t_sun` passes straight through and
the convection stops stepping.
