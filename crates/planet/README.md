# planet

One planet filling the frame — the hero framing of
[`planet-core`](../planet-core/README.md), with 26 types and a full tuning panel on
the web.

The crate's `lib.rs` is 18 lines: the algorithm lives in the rlib that `solar` and
`moon` render their bodies with. What is here is the framing, the native generator,
the C-ABI face and the demo page.

```bash
cargo run --release -p planet --bin planet    # GIFs + contact sheet into out/

cargo build -p planet --target wasm32-unknown-unknown --release --no-default-features
cp target/wasm32-unknown-unknown/release/planet.wasm crates/planet/web/planet.wasm
cd crates/planet/web && python3 -m http.server 8000
```

## Web controls

Type · Seed · Resolution · Spin, then live sliders for every parameter — contrast,
frequency, ice caps, specular, shininess, glare↔albedo, clouds, storm cells,
aurora, lightning, great spot, bands, turbulence — plus a **Look** section: palette
swap, dither, orbiting-moons toggle, CRT/scanline toggle. Sliders snap to each
type's defaults on selection.

A **Renderer** dropdown switches between the wasm CPU path and WebGL2. It defaults
to the GPU and falls back with the reason shown when WebGL2 is missing.

## Feature cost lab

An ablation panel: a tick-box per shader feature, and a button that switches each
one off in turn and times the difference. It measures on *your* machine, so the
numbers in [`planet-core`](../planet-core/README.md#what-each-feature-costs) are a
reference point rather than a claim about your hardware.

It also reports what the two optimizations are worth in this framing: cheap warp
saves 19% on `terran`, 31% on `gas_giant`, 51% on `storm_shroud`; night-side
thinning saves 4% on `terran`, 11% on `ocean`.

`cargo run --release -p planet --bin bench` is the native version of the same
measurement.

## The worker pool

A wasm *instance* is one thread, so a single instance uses exactly one core however
many the machine has — and that, not the instruction set, is the largest gap left
to native. Wasm here runs within ~1.3x of native single-threaded.

This demo fans a frame across a **worker pool**: the shader is a pure function of
pixel position, so each worker renders a horizontal band into its own buffer and
transfers it back with no copy. That needs no COOP/COEP headers (unlike
`SharedArrayBuffer`, which GitHub Pages can never provide), so it works on Pages.

In this container (4 shared cores), a 256px planet with every switch on:

| | ms/frame | fps |
|---|---:|---:|
| 1 worker | 17.79 | 56 |
| 4 workers | 6.10 | 164 |

**2.9x at 73% of four cores**, with 0.18 ms of dispatch against a 6 ms frame — so
band-splitting costs ~3% to set up, not enough to eat the win.

`scripts/make-parallel-probe.sh` builds a single self-contained page that measures
this on a given host before anyone writes a pool: whether a strict CSP forbids
blob-URL workers (the only way a one-file build gets off the main thread), whether
`simd128` is there, and what the cores are really worth. It runs the actual
`planet.wasm`, not a synthetic loop, and prints throughput and dispatch separately
on purpose — throughput is the optimistic number, dispatch is the part it hides.

A pool only pays because the workers render **standalone**. A pooled *scene* was
written and rejected: shipping each band's backdrop rows into a worker and the
finished strip back out is ~4.3 MB of copying a frame at 900x600, which swamps the
~2 ms of body shading the split saves. See the
[root README](../../README.md#rendering-on-the-gpu).

> Headless virtual time starves workers: `chromium --headless
> --virtual-time-budget` fast-forwards the main thread's timers but not worker
> threads, so the pooled path never completes a frame and reads as a blank canvas.
> Set the pool to 0 before concluding anything about the CPU renderer that way.
