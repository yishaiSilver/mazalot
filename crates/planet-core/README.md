# planet-core

**The** planet renderer — the only one in the workspace. The 26-type table, sphere
shading, weather, rings, moons, and the GLSL port of all of it.

`planet` frames it head-on, `solar` puts it in orbit, `moon` hangs moons in front
of it. If you find yourself writing a "simpler" planet shader for a new crate, add
a framing here instead — that duplication has been removed once already.

## Two framings, one shader

- **`render_rgba*`** — hero: a planet filling a square frame, fixed key light,
  starfield.
- **`render_tile`** / **`render_tile_into`** — scene: cut out on transparency,
  sized to its disc, lit from any direction, ready to `blit`.

## 26 types across 5 base algorithms

Terrestrial (terran, ocean, archipelago, desert, swamp, iron, ice, savanna, gaia,
tundra, alpine, obsidian, chrome), cratered (barren, moon), banded
gas/ice/storm/ringed giants, emissive (lava, molten sea, radioactive, fungal,
crystal), and cloud-shrouded (toxic, storm shroud) — plus **rings**, **orbiting
moons**, and material-aware **specular glare**.

**Adding a type** is one row in `TYPES` (`src/lib.rs`): palette, thresholds, flags.
The native GIFs, the web demo and the GPU path all pick it up automatically. To put
the world into orbit too, add it to `ROSTER` in [`solar`](../solar/README.md).

## Fake 3D

Treat each disc pixel as the front hemisphere of a unit sphere, rotate the surface
point around Y by the spin angle, and sample **3D noise** there. Shade with a light
(Lambert + Blinn-Phong specular scaled by local albedo) and an atmosphere rim.
Sampling in 3D means no seams and no pole stretching, and a full 360° spin loops
seamlessly.

The light-direction basis is **+x right, +y up, +z toward viewer**. It is easy to
get wrong.

## Animated weather (loop-safe)

- **Clouds** — domain-warped wispy fronts that drift and billow, casting soft shadows.
- **Gas-giant bands** — counter-rotating zonal jets + domain warp (fluid, not a sine wobble).
- **Great spot** — a drifting spiral cyclone with a calm eye.
- **Lightning** — small irregular flashes on storm worlds.
- **Aurorae** — polar curtains, hue palette-cycled green→cyan→violet.
- **Storm cells** — bounded hurricane swirls in the cloud layer.
- **Molten flow** — palette-cycled glow flowing across emissive worlds.

Output goes through [`dither-core`](../dither-core/README.md): ordered dither,
limited palettes (`Natural`, `Game Boy`, `Ice`, `Sunset`), and a 1-px dark rim on
every disc and moon for sprite readability.

## Lod: octaves are derived, not fixed

A tile puts one sphere radius across `rad` px, so a field sampled at `p · freq`
lands its `k`-th octave at `rad / (freq · 2^(k-1))` px. Under two pixels it is past
Nyquist — it cannot be resolved, and since the planet turns, what it contributes is
crawling speckle. `Lod` drops those octaves.

This is a mip level, not a quality knob: a planet 20 px across pays for three
octaves, the same planet filling the screen pays for six, and coastlines come out
*steadier* than before.

The consequence to remember: **the same planet at a different on-screen size is
legitimately different pixels.** A change that moves a body's radius will move its
imagery. What must stay stable is a body at a *fixed* radius.

## Cost

Per-pixel procedural: every frame recomputes noise for every pixel. A sprite is a
`memcpy`; a planet is thousands of times more expensive. Native (wasm in-browser
runs ~2–3× slower):

| @ 64px | per frame | vs a sprite |
|---|---|---|
| sprite blit (`memcpy`) | ~0.0002 ms | 1× |
| planet, no weather (iron) | 0.45 ms | ~2,500× |
| planet, full weather (terran) | 1.49 ms | ~8,400× |
| lava (emissive) | 0.64 ms | ~3,600× |

**The weather is the cost** — domain warp on clouds and bands roughly triples the
base planet. **The pixel-art pipeline is nearly free:** dithering, moons and
palette swaps add < 0.05 ms together.

Cost is quadratic in rendered size, but not quite — `Lod` takes some of it back.

- **One planet live**: comfortable — ~1.5 ms native, ~4–5 ms in wasm at 64 px.
  Tightens above ~200 px.
- **Many planets / a galaxy map**: don't render live. Bake the ~30 spin frames
  once, then blit — procedural variety at sprite-cheap playback.
- **Cheaper weather**: dropping domain warp back to plain fBm roughly halves it.

### Zoomed in

A planet cannot be tile-cached the way a star is: its axial rotation *is* the
visible motion, so at any usable quantum the tile changes every frame. The
zoomed-in case is attacked by doing less work per frame instead — `Lod` above,
[`scene-core`](../scene-core/README.md)'s `visible_tile_rect` clip and run-walking
`blit`, and `noise_core::fbm_warp_oct`, which caps a warp's displacement field at
two octaves whatever the size (a 4-octave `fbm_warp`'s 16 octave evaluations become
10).

In the browser (V8, 1680×944 buffer, seed 7, detail cap 64):

| | before | after |
|---|---|---|
| planet filling the viewport | 28.2 ms (35 fps) | **17.3 ms (58 fps)** |
| planet at 2× the viewport | 31.6 ms (32 fps) | **9.4 ms (106 fps)** |
| mid-zoom, several worlds past the cap | 39.2 ms (25 fps) | **17.1 ms (58 fps)** |
| compositing one full-screen body | 10.6 ms | **2.2 ms** |
| surface shader, median archetype | 633 ns/px | **361 ns/px** |

**The detail cap is the biggest single lever**, and it sets a *radius*: cost goes
as its square, so halving the slider quarters the shader work. At one zoom, cap 32
is 5.8 ms, cap 64 is 17.8 ms, cap 160 is 95 ms. The shipped default of 160 buys
full six-octave detail on a screen-filling world and costs accordingly.

### What each feature costs

`planet`'s ablation panel switches each shader feature off in turn and times the
difference — see [`planet`](../planet/README.md#feature-cost-lab). `terran` at 64²
(full frame 1.449 ms):

| feature | cost | share |
|---|---:|---:|
| **cloud layer** (all of it) | 0.801 ms | **55%** |
| ├ cloud colour (`fbm_warp`) | ~0.53 ms | 37% |
| ├ **cloud self-shadow** | 0.176 ms | **12%** |
| └ storm-cell swirl | 0.093 ms | 6% |
| **aurora** | 0.172 ms | **12%** |
| **specular + shimmer** | 0.133 ms | **9%** |
| atmosphere rim · lightning · ice caps · great spot · moons | ≤0.03 ms each | ≤2% |
| ordered dither · dark limb · starfield | ~0 | ~0% |

The cloud deck is the whole game on a terrestrial world. Two poor-value items:
**aurora costs 12% for a thin polar band**, and **specular is 19% of a `lava`
frame** for a glint of intensity 0.05. By archetype: `gas_giant` is great spot 16%,
`lava` is specular 19% and nothing else, `barren` has nothing above 5% because it
is all Worley, which has no switch.

### The `F_*` mask

Most features are reachable through the per-type sliders — `clouds`, `specular`,
`spot`, `aurora`, `lightning`, `storm_cells` and `caps` are gated on `> 0.0`, so
zeroing one switches it off. `F_*` covers what a parameter cannot reach: part of a
layer (the cloud self-shadow), framing furniture (atmosphere rim, dark limb,
starfield), and the two optimizations.
`render_rgba_features(.., F_ALL, ..)` is byte-identical to `render_rgba_styled`.

**`F_ALL` is not every `F_*` bit.** It is the five that leave the picture alone.
`F_NIGHT_LOD` was in it until `Lod` started feeding the aurora and the great spot
as well as the base field — capping octaves past the terminator then moved pixels,
and `out/` caught it. **If a switch changes the image, it lives outside `F_ALL`**,
callers that want it opt in (`System::night_lod`, `MoonSystem::night_lod`, both
`false` by default), and `out/` stays byte-identical.

## WebGL2

`src/shader.glsl` re-implements the pixel loop for WebGL2, concatenated after
`noise-core`'s prelude. It is ~200 lines of ramps, mixes and smoothsteps —
everything numeric stays in Rust. `gl_uniforms()` computes the `PType` row with
slider overrides applied, the seed offsets, the vortex centres, this frame's moons,
the key light, the colour ramp, the palette and the whole `Lod` octave budget, and
ships them as one flat float array. **The type table is transported, not
duplicated** — a new type is still one row, and the GPU picks it up.

The shader source lives inside the wasm module (`gl_src_ptr`/`gl_src_len`), so it
cannot go stale against the module it was built with, and the single-file artifact
keeps working.

It renders `F_ALL`, deliberately *without* `F_NIGHT_LOD`: that switch buys a CPU
back octaves it cannot afford and moves pixels on the dark limb doing it. A
rasterizer does not need the trade, and leaving it off is what lets `verify-gl.mjs`
compare the two renderers pixel for pixel rather than approximately.

**The `U_*` slots are a wire format.** The `#define`s in the GLSL and the `GL_U_*`
constants in Rust must agree; `glsl_slot_indices_match_the_rust` parses the
`#define`s and pins them, because a slot off by one paints a planet with somebody
else's colours rather than failing. It pins **16 of the 66** `#define`s. The `O_*`
octave slots are unpinned and are equally a wire format — `gl_octaves` writes them
positionally — so renumbering one hands the aurora the crater's octave count,
silently. **Pin a slot when you add one.**

`mod gl` is `#[cfg(any(feature = "gl", test))]`, and that gate is load-bearing, not
tidiness — see [Float codegen](../../README.md#gotchas).

### Verification

All 26 types, 4 angles × 2 seeds, 128px, CPU vs GPU:

| | |
|---|---|
| types bit-identical to the wasm renderer | **15 of 26** |
| worst per-type pixel disagreement | **0.09%** (`ocean`) |
| pixels differing by more than one quantization level | **0.00%** |

Every differing pixel differs by exactly one 22-level step (12/255) — the signature
of a value landing on the other side of a quantizer threshold, not a shading bug.
ANGLE is free to round `sin`, `exp`, `pow` and `sqrt` its own way, and a 1e-7
difference before `quant` becomes a whole level after it. The types that come out
*exactly* equal are the ones whose shading is ramps and steps with no transcendental
in the path — `barren`, `moon`, `lava`, `desert`, `chrome`.

```bash
node scripts/verify-gl.mjs --demo planet --types all
```

Run it after touching either shader. Read the right column: pixels differing by
more than one quantization level are the signal.
