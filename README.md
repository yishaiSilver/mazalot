# mazalot

<p align="center">
  <img src="docs/solar.gif" width="480" alt="A procedurally generated solar system: four worlds orbiting a blue-white star on dotted elliptical paths, against a parallax starfield.">
</p>
<p align="center">
  <em>One seed, no art assets. Regenerate with <code>cargo run --release -p solar --bin solar</code>.</em>
</p>

Procedural, seed-driven pixel-art sprite generators in Rust — **zero art assets**.
Every planet, star, comet, asteroid field and creature is math evaluated per pixel,
so a seed always rebuilds the identical image. Each generator compiles twice from
one source: a native GIF/PNG bin, and a ~56 KB raw-C-ABI WebAssembly module driving
a browser demo.

## The crates

Each has its own README with the detail.

**Demos** — what you can run:

| Crate | |
|---|---|
| [`planet`](crates/planet/README.md) | One planet filling the frame: 26 types, a full slider panel, an ablation lab and a worker pool. |
| [`star`](crates/star/README.md) | One star filling the frame: granulation, sunspots, prominences, corona, 8 spectral types. |
| [`solar`](crates/solar/README.md) | A draggable, zoomable solar system — a star with worlds in eccentric orbit, on the CPU or the GPU. |
| [`moon`](crates/moon/README.md) | A world with moons orbiting it, depth-sorted so they pass in front of and behind. |
| [`comet`](crates/comet/README.md) | A comet on an eccentric orbit, its anti-sunward tail swinging through perihelion. |
| [`asteroid`](crates/asteroid/README.md) | A drifting, perspective-squashed asteroid belt. |
| [`character`](crates/character/README.md) | A paper-doll character compositor (native only). |
| [`bird`](crates/bird/README.md) | A disjoint creature generator: hybrid aliens and naturalistic earth birds. |

**Libraries** — the shared machinery, no third-party dependencies:

| Crate | |
|---|---|
| [`noise-core`](crates/noise-core/README.md) | 3D value-noise, fBm, domain warp, Worley and colour math — four-lane on wasm `simd128`, and the bottom of everything. |
| [`dither-core`](crates/dither-core/README.md) | Bayer ordered dither and level quantization: the pixel-art output stage. |
| [`scene-core`](crates/scene-core/README.md) | The compositor kit — draggable `Camera`, seeded `Rng`, `Tile` + `blit`, and the clip that doubles as the visibility test. |
| [`background-core`](crates/background-core/README.md) | **The** backdrop: dithered ground, seeded nebula, parallax star layers, and the cache a pan scrolls instead of rebuilding. |
| [`planet-core`](crates/planet-core/README.md) | **The** planet renderer: 26-type table, sphere shader, weather, rings, moons, and the GLSL port of all of it. |
| [`sun-core`](crates/sun-core/README.md) | The compact star tile — granulation plus a corona that is tabulated rather than shaded. |
| [`wasm-abi`](crates/wasm-abi/README.md) | `alloc`/`dealloc` and opaque-handle macros for the raw C ABI. No wasm-bindgen. |
| [`render-io`](crates/render-io/README.md) | The only crate that touches `image`: GIF/poster helpers and the parallel GIF encoder. |

## Architecture

Every **demo crate** has the same three faces:

```
crates/<name>/src/lib.rs    pure render math (rlib for the bins, cdylib for wasm)
crates/<name>/src/wasm.rs   thin C-ABI wrapper, #[cfg(target_arch = "wasm32")]
crates/<name>/src/bin/*.rs  native generators, behind the `native` feature
crates/<name>/web/          the browser demo (index.html + a committed .wasm)
```

The library layer is 8 crates / 1,886 lines and stacks in one direction only:

```
                          render-io ──── image, gif, rayon (the only third-party deps)
                          wasm-abi  ──── (nothing)

noise-core ──┬── dither-core ──┬── background-core ── solar, moon, comet, asteroid
             │                 │
             └── scene-core ───┼── planet-core ────── planet, solar, moon
                               │
                               └── sun-core ───────── solar, comet
```

● declared in that crate's `Cargo.toml` · ○ transitive · `render-io` is always
behind the `native` feature, so the wasm build never sees it.

| library crate      | lines | planet | star | solar | moon | comet | asteroid |
|--------------------|------:|:------:|:----:|:-----:|:----:|:-----:|:--------:|
| `noise-core`       |   618 |   ○    |  ●   |   ●   |  ●   |   ●   |    ●     |
| `dither-core`      |    31 |   ○    |  ●   |   ○   |  ●   |   ●   |    ●     |
| `scene-core`       |   130 |   ○    |  ·   |   ●   |  ●   |   ●   |    ●     |
| `background-core`  |   365 |   ·    |  ·   |   ●   |  ●   |   ●   |    ●     |
| `planet-core`      |   815 |   ●    |  ·   |   ●   |  ●   |   ·   |    ·     |
| `sun-core`         |   124 |   ·    |  ·   |   ●   |  ·   |   ●   |    ·     |
| `wasm-abi`         |    87 |   ●    |  ●   |   ●   |  ●   |   ●   |    ●     |
| `render-io`        |   240 |   ●    |  ●   |   ●   |  ●   |   ●   |    ●     |
| **`lib.rs`**       |       | **18** | 567  |  767  | 503  |  557  |   490    |
| **`wasm.rs`**      |       |   93   |  58  |  166  |  76  |   79  |    71    |

Three rules hold the shape:

- **One renderer per thing.** `planet-core` is the only planet shader,
  `background-core` the only sky. A new framing goes into the core crate; a
  "simpler" copy in a new crate has been removed once already.
- **Demo crates never depend on each other.** They are cdylibs whose `#[no_mangle]`
  exports collide at link time in the wasm build. Share through an rlib — which is
  why `planet` is 18 lines over `planet-core`.
- **Library crates stay third-party-free.** The wasm build is
  `--no-default-features`, so anything reachable from `lib.rs` without the `native`
  feature ships in the module. `image` and `rand` live behind `render-io`.

## Running it

**Native — GIFs and PNGs into `out/`:**

```bash
cargo run --release -p planet    --bin planet
cargo run --release -p star      --bin sun        # note: the bin is `sun`
cargo run --release -p solar     --bin solar
cargo run --release -p moon      --bin moon
cargo run --release -p comet     --bin comet
cargo run --release -p asteroid  --bin asteroid
cargo run --release -p character --bin character
cargo run --release -p bird      --bin alien      # the disjoint half
cargo run --release -p bird      --bin bird
```

**Web — any demo.** Build the cdylib, drop it beside its page, serve:

```bash
c=solar    # or planet, star, moon, comet, asteroid, bird
cargo build -p $c --target wasm32-unknown-unknown --release --no-default-features
cp target/wasm32-unknown-unknown/release/$c.wasm crates/$c/web/$c.wasm
cd crates/$c/web && python3 -m http.server 8000   # http://localhost:8000/
```

Needs `rustup target add wasm32-unknown-unknown`. `--no-default-features` drops the
native-only `image`/`rand`. **Run wasm builds from the repo root** — `simd128` comes
from `.cargo/config.toml`, cargo reads it from the working directory, and without it
the noise kernels are slower than the scalar code they replaced.

`scripts/make-artifact.sh solar` bundles a demo into one self-contained HTML with
the wasm inlined as base64 — no server needed, hostable anywhere. See
[docs/artifacts.md](docs/artifacts.md).

## Rendering on the GPU

Both `planet` and `solar` default to **WebGL2** and fall back to the wasm CPU
renderer (with the reason shown) when it is missing. Four `.glsl` files are the
sanctioned second implementation: `noise-core`'s prelude, plus `planet-core`'s,
`background-core`'s and `sun-core`'s bodies.

The port is far cheaper than it sounds, for one reason:

> **`hash3` and `value_noise` are `u32` integer math.** Wrapping multiplies, xors,
> shifts — nothing a driver is free to round its own way. They transliterate into
> GLSL ES 3.00 *exactly*, so the lattice under the GPU picture is bit-identical to
> the lattice under the CPU one, and Worley and the fBm stack fall out for free.

So what is rewritten is the shading, not the maths. Each crate's `gl_uniforms()`
computes its tables, seeded constants and octave budgets in Rust and ships them as
one flat float array, so `TYPES`, `SUNS` and `STAR_LAYERS` are **transported, not
duplicated** — a new planet type is still one row, and the GPU picks it up. A GPU
scene is a **draw list, not pixels**: `solar::gl_bodies` emits one record per body,
sorted back-to-front, and the JS draws a quad each with alpha blending, which is
what `blit` was doing by hand.

Going to the GPU was not about dividing the frame faster. A worker pool splits a
scene's *bodies* across cores, but the backdrop is full-screen serial work that
scales with the window, and a camera following a planet invalidates its cache every
frame — an Amdahl term no number of workers removes. A pooled `solar` measured 1.13x
at best and worse than nothing with an empty sky, because band traffic (~4.3 MB per
frame at 900x600) cost more than the body shading it saved. Moving the whole frame
deletes the serial term instead, and takes the pixel plumbing with it — nothing is
read back at all.

**Scatter, don't gather.** The first backdrop shader had every pixel test nine cells
in each of three star layers — 27 hashes per pixel against roughly one per fifty —
and it was three quarters of the fragment cost (216.6 ms/frame, vs 50.0 with the
stars as point sprites). Before writing a gather into any shader, check whether the
vertex path will do. Details in
[background-core](crates/background-core/README.md#scatter-dont-gather).

**The GPU is checkable here, not measurable.** This container has no `/dev/dri`, so
the only WebGL2 available is ANGLE over SwiftShader — a software rasterizer. It
proves the shader is right and says nothing about GPU throughput; a timing taken
through it is a timing of the CPU.

## Verifying a change

There are no unit tests for the rendering itself — **the generated images are the
test.**

```bash
cargo build --release --workspace
for c in planet solar moon comet asteroid bird character; do
  cargo run -q --release -p "$c" --bin "$c"
done
cargo run -q --release -p star --bin sun     # the bin is `sun`, not `star`
cargo run -q --release -p bird --bin alien   # bird has two bins; this one is easy to miss
(cd out && sha256sum *) | sort > /tmp/after.sha256
```

`out/` is gitignored and holds 74 files. Hash it **before** you touch anything,
again after, and diff. A refactor meant to be behaviour-preserving should come out
byte-identical; if something changed, you must be able to name which crate and why.

Then, as applicable:

- **`cargo test --workspace`** — roster tests and the GLSL wire-format checks. Fast;
  run it.
- **`node scripts/verify-gl.mjs --demo all`** — the GL path's equivalent of `out/`.
  It renders both paths in headless Chromium and diffs per pixel. Needs
  `npm i -g playwright`, and **nothing runs it for you** — no CI. Read the right
  column: pixels differing by *more than one quantization level* are the signal
  (0.00% today); one-level differences are ANGLE rounding a `sin` differently across
  a `quant` threshold. Its pass gate is a *rate* (0.5%), so read the warning when
  the rate is non-zero.
- **The wasm export set**, which is the contract with the JS and breaks a demo
  silently — see [wasm-abi](crates/wasm-abi/README.md#checking-the-export-set).
- **The wasm render path**, whenever you touch `noise-core`: `out/` only covers
  native. Instantiate the module in node with `{}`, call `alloc`/`render`/`dealloc`,
  hash the pixels.

The committed `crates/*/web/*.wasm` files go stale easily. Change a crate's render
path, rebuild and copy the wasm over.

## Gotchas

- **Float codegen is load-bearing.** Moving code between crates changes LTO and FMA
  contraction, which shifts pixels by a few /255 across dither thresholds — not a
  logic bug, but it *will* break byte-identity. Quantify the delta before deciding
  it is fine.
- **...and you do not have to *move* code to trigger it.** `gl_uniforms` computes no
  pixels, but merely existing in `planet-core` as another caller of `Lod::oct` and
  friends re-priced their inlining and moved `out/moon_*.png` by up to 4/255 across
  5% of its pixels — while five other crates stayed byte-identical, which is what
  makes it easy to miss. `mod gl` is therefore gated behind the `gl` feature (plus
  `test`), so the native generators never compile it.
- **Octave counts are derived from on-screen size** (`planet_core::Lod`), so the same
  planet at a different radius is legitimately different pixels. What must stay
  stable is a body at a *fixed* radius.
- **Benchmark with a control.** Timings on this machine swing ±60% between runs.
  Build the baseline in a throwaway `git worktree`, interleave the two binaries in
  one loop, and use an untouched pass as the control. Watch that the worktree build
  really has `simd128`.
- **Headless virtual time starves workers**, so a pooled CPU path reads as a blank
  canvas under `chromium --headless --virtual-time-budget`. Set the pool to 0 first.
- `cargo build --workspace` warns about a `bench` output-filename collision between
  `planet` and `solar`. Pre-existing; ignore it.
