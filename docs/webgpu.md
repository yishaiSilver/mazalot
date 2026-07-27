# The WebGPU planet path

The planet shader is per-pixel and embarrassingly parallel: every pixel reads the
type table, samples noise, and writes one RGBA value, with no dependency on any
other pixel. The `planet` demo therefore ships it twice —

- **CPU** — `planet_core::render_rgba_styled`, compiled to wasm. The reference
  implementation, and what the native GIF/PNG bins and `solar`'s sprite tiles use.
- **GPU** — `crates/planet-core/src/planet.wgsl`, a fragment shader, one
  invocation per pixel. Used by `crates/planet/web/index.html` when the browser
  has WebGPU.

The demo picks the GPU at load and silently falls back to the CPU path when
WebGPU, an adapter, a device, or a clean shader compile isn't available. The
badge over the canvas says which one is live. **No configuration, and no way for
the page to end up blank** — every failure in `initGPU()` returns `null`.

## Why it exists

Not for the native generators — those are already fast enough and must stay
byte-reproducible. It exists so the *demo* can run at a resolution that reads as
a real planet instead of a 64×64 thumbnail. The resolution slider caps at 240 on
the CPU path and 512 on the GPU path, which is the whole point.

## What is and isn't duplicated

The **algorithm** is duplicated — there is no way around that short of running
Rust on the GPU. The **data** is not:

```
planet_core::TYPES  ──gpu::type_table()──►  f32[26][76]  ──storage buffer──►  planet.wgsl
```

Adding a planet archetype stays a one-row edit in `TYPES`; the GPU picks it up
with no shader change. The same is true of the 13 slider parameters, which
travel in the per-frame buffer rather than being re-declared in WGSL.

Three things guard the seam, all runnable without a GPU:

| check | where | catches |
| --- | --- | --- |
| `gpu::tests::shader_offsets_match` | `cargo test -p planet-core` | a field offset in `gpu.rs` drifting from `planet.wgsl` |
| `gpu::tests::stops_fit` | `cargo test -p planet-core` | a new colour ramp outgrowing the fixed GPU slot |
| the `webgpu contract` block | `node crates/planet/web/verify.mjs` | `index.html`'s frame layout drifting from the shader's |

They matter because the buffers are untyped floats: a drifted offset renders a
*wrong* planet rather than failing, and only on machines that have WebGPU.

## Fidelity

The two paths are not contractually byte-identical — `fma` contraction, `sin`/
`pow` precision and rounding mode all differ between a CPU and a GPU. In practice
they land much closer than that:

Measured against the wasm CPU path at 64², all 26 types at seed 1 / angle 0.7,
plus palette and ringed variants (30 cases):

- **28 of 30 cases byte-identical.**
- The other two (`terran`, `archipelago`) differ on **0.02% of pixels** — a
  couple of pixels each, by one quantization level, where a value sits exactly on
  a hard `ramp()` stop threshold and the two evaluations fall on opposite sides.

Getting there needed one deliberate adjustment, which is worth knowing if you
touch the output stage: the CPU path writes `(v * 255.0) as u8`, which
**truncates**, while writing to an `rgba8unorm` target **rounds to nearest**.
Left alone that put ~58% of pixels one byte above the CPU's. The shader
pre-truncates with `floor(px * 255.0) / 255.0` so the target's rounding has an
exact integer to land on.

`planet.wgsl` implements the **hero framing only**. `render_tile` — the
transparent sprite framing `solar` blits — stays CPU-side, where the scene
compositor lives.

## WGSL traps this port hit

Worth reading before editing the shader; each of these is a silent wrong-pixels
bug, not a compile error.

- **`smoothstep` is not `noise_core::smoothstep`.** WGSL's is undefined when
  `e0 >= e1`, and the shader calls it that way on purpose all over the place
  (`sstep(1.0, 0.15, d)`) to get a falling ramp. Hence the local `sstep`.
- **`mix` is not `lerp`.** WGSL's `mix` is `a*(1-t) + b*t`; Rust's `lerp` is
  `a + (b-a)*t`. They round differently. Hence the local `lerpf`/`lerp3`.
- **`round` breaks ties to even**; Rust's `f32::round` breaks them away from
  zero. `quant` uses `floor(v + 0.5)`.
- **`i32`→`u32` conversion** is written as `bitcast`, to match Rust's `as u32`
  on negative values.

## Developing against it

There is no GPU in CI, and headless Chromium here ships without WebGPU. Deno's
WebGPU (backed by Mesa's `lavapipe` software Vulkan) does work and is how the
numbers above were measured:

```bash
apt-get install -y mesa-vulkan-drivers   # software Vulkan
npm i -g deno
deno run --unstable-webgpu -A parity.ts  # see git history / scratch for the harness
```

The harness worth rebuilding if you need it extracts the `<script type="module">`
block straight out of `index.html` and calls the page's own `initGPU()`/`draw()`
against a stubbed canvas, so it tests the shipped wiring rather than a copy. One
trap: the stub's `getCurrentTexture()` must create its texture from the device
handed to `configure()`. Returning a texture from a *different* device is a
cross-device error that WebGPU surfaces as a silently empty frame, which looks
exactly like a broken shader.
