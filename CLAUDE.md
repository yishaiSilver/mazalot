# mazalot — notes for agents

Procedural, seed-driven **pixel-art sprite generators** in Rust. Zero art assets:
every planet, star, comet, asteroid field and creature is math evaluated per
pixel, so a seed always rebuilds the identical image. Each generator compiles
twice from one source — a native GIF/PNG bin, and a raw-C-ABI WebAssembly module
driving a browser demo.

`README.md` is the reference for *what* the algorithms do. This file is about
*how the repo works* and what will bite you.

## Layout

A Cargo workspace under `crates/`, in two layers.

**Library crates** hold shared machinery. They carry **no third-party
dependencies** (except `render-io`, which is the one that owns `image`):

| crate | what |
|---|---|
| `noise-core` | 3D value-noise, fBm, domain warp, Worley, colour/ramp math. Bottom of everything. |
| `dither-core` | Bayer ordered dither + level quantization. |
| `scene-core` | `Camera`, seeded `Rng`, `Tile` + `blit` alpha compositor. |
| `background-core` | **The** backdrop — dithered ground, optional seeded nebula (baked + cached), parallax star layers. |
| `planet-core` | **The** planet renderer — 26-type table, sphere shader, weather, rings, moons. |
| `sun-core` | The compact star tile (granulation + corona). |
| `wasm-abi` | `alloc`/`dealloc` + opaque-handle macros for the C ABI. |
| `render-io` | GIF/contact-sheet/poster helpers. The only crate that touches `image`. |

They stack in one direction only — `noise-core` → `dither-core`/`scene-core` →
`background-core`/`planet-core`/`sun-core`. See the import table in `README.md`.

**Demo crates** — `planet`, `star`, `solar`, `moon`, `comet`, `asteroid`, `bird` —
all have the same three faces:

```
crates/<name>/src/lib.rs    pure render math (rlib for the bins, cdylib for wasm)
crates/<name>/src/wasm.rs   thin C-ABI wrapper, #[cfg(target_arch = "wasm32")]
crates/<name>/src/bin/*.rs  native generators, behind the `native` feature
crates/<name>/web/          the browser demo (index.html + a committed .wasm)
```

`character` is the exception: native-only, no wasm, no lib. `bird` is fully
disjoint from the space crates — it shares nothing but third-party deps.

## Hard constraints

**One planet renderer.** `planet-core` is the single source of truth for what a
planet looks like. It exposes the same shader in two framings:

- `render_rgba*` — hero: a planet filling a square frame, fixed key light, starfield
- `render_tile` — scene: cut out on transparency, sized to its disc, lit from any
  direction, ready to `blit`

`planet` frames it head-on, `solar` puts it in orbit, `moon` hangs moons in front
of it. If you find yourself writing a "simpler" planet shader for a new crate,
add a framing to `planet-core` instead — that duplication has been removed once
already.

**Bodies go through the compositor's clip, not a hand-rolled cull.** A scene
draws a body by asking `scene_core::visible_tile_rect` where its tile lands, and
passing that rect back into `render_tile_clipped` / `render_tile_into` /
`render_star_tile_into`. Two things follow, and both matter:

- An **empty rect is the visibility test** — exact, and free. Do not add a
  "body radius × some margin" off-screen check next to it. There was one; it had
  to over-estimate for ringed giants and corona halos, so it kept rendering
  full-price tiles for bodies that were entirely off-screen.
- Pixels **outside the rect are not shaded**, so a tile is only valid for the
  placement its clip came from. Anything that caches a tile must put the clip in
  the cache key (`SunCache` does) — and then snap the rect outward to a grid
  (`snap_out`), or a camera drifting a pixel a frame invalidates the cache every
  frame and the caching buys nothing.

`scene-core`'s tests pin the rect as a superset of what `blit` reads; a rect
that under-reports by a pixel leaves an unshaded seam only visible at the zoom
levels nobody screenshots.

**One backdrop, likewise.** Every scene paints through `background-core`:
`paint_backdrop` (ground + optional nebula) then `paint_stars`. A new scene crate
supplies a `Backdrop` and a `Starfield` const and a closure that mixes its seed
into the star grid — it does not write another star loop. The four that existed
differed only in constants, which is how they silently drifted apart.

**Demo crates never depend on each other.** They are cdylibs whose `#[no_mangle]`
exports (`render`, `alloc`, `dealloc`, …) collide at link time in the wasm build.
Share through an rlib. This is why `planet-core` exists as a crate separate from
`planet`.

**Library crates stay third-party-free.** The wasm build is
`--no-default-features`; anything reachable from `lib.rs` without the `native`
feature ends up in the module. Keep `image`/`rand` behind `native` and behind
`render-io`.

**Rosters name archetypes by string.** `solar::ROSTER` and `moon::PARENTS` refer
to `planet-core` types by name (`"gas_giant"`, `"terran"`), resolved via
`planet_core::type_index`. A typo silently falls back to type 0 — the tests in
those two crates exist to catch exactly that. Keep them passing.

**Web name arrays are hand-synced.** `PLANET_NAMES` in `crates/solar/web/index.html`
and `PARENT_NAMES` in `crates/moon/web/index.html` are index-aligned with the Rust
rosters; the C ABI can't return strings. Reorder a roster, edit the HTML.

## Verifying a change

There are no unit tests for the rendering itself — **the generated images are the
test.** The workflow that catches regressions:

```bash
cargo build --release --workspace
for c in planet solar moon comet asteroid bird character; do
  cargo run -q --release -p "$c" --bin "$c"
done
cargo run -q --release -p star --bin sun     # note: the bin is `sun`, not `star`
(cd out && sha256sum *) | sort > /tmp/after.sha256
```

`out/` is gitignored and holds 94 files. Hash it **before** you touch anything,
again after, and diff. A refactor that is supposed to be behaviour-preserving
should come out byte-identical; if something changed, you must be able to name
which crate and why.

For the wasm side, check the C-ABI export set is unchanged — that is the contract
with the JS:

```bash
cargo build -p solar --target wasm32-unknown-unknown --release --no-default-features
```

Then compare the exported `func` names before/after. Changing them breaks a demo
silently, because the JS calls them by name.

`cargo test --workspace` runs the handful of roster tests. It is fast; run it.

## Gotchas

- **Octave counts are derived, not fixed.** `planet_core::Lod` picks each fBm
  field's octave count from the disc's pixel radius, dropping octaves whose
  lattice cell falls under two pixels (past Nyquist — they can't be resolved, and
  on a turning planet they read as crawling speckle). So the *same planet at a
  different on-screen size is legitimately different pixels*, and a change that
  moves a body's radius will move its imagery. That is working as intended; what
  must stay stable is a body at a **fixed** radius.
- **Float codegen is load-bearing.** Moving code between crates changes LTO and
  FMA-contraction decisions, which shifts pixels by a few /255 across dither
  quantization thresholds. This is not a logic bug, but it *will* break
  byte-identity. Quantify the delta (max per-channel difference) before deciding
  it's fine.
- **Benchmark with a control.** This machine's timings swing ±60% between runs.
  Build the baseline in a throwaway `git worktree` and interleave the two binaries
  in one loop, using an untouched pass (e.g. solar's background) as the control.
- `cargo build --workspace` warns about a `bench` output-filename collision between
  `planet` and `solar`. Pre-existing; ignore it.
- `scripts/make-artifact.sh <crate>` bundles a demo into one self-contained HTML
  with the wasm inlined as base64. It rebuilds the wasm unless given `--no-build`.
- The committed `crates/*/web/*.wasm` files go stale easily. If you change a
  crate's render path, rebuild and copy the wasm over.

## Adding things

- **A planet type** — one row in `TYPES`, `crates/planet-core/src/lib.rs`. It shows
  up in the `planet` demo automatically. To put it in orbit too, add it to
  `solar::ROSTER` with an orbital band.
- **A star type** — one row in `STYPES`, `crates/star/src/lib.rs`.
- **A star for a scene** — one row in `SUNS`, `crates/solar/src/lib.rs`.

## Style

Match the surrounding code. It is densely commented, and the comments explain
*why* a constant is what it is ("0.375 leaves orbital margin for moons and rings"),
not what the line does. Keep that. Doc-comment public items with what a caller
needs, including units and coordinate conventions — the light-direction basis
(+x right, +y up, +z toward viewer) is easy to get wrong.
