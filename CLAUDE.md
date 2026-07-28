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
dependencies** (except `render-io`, which owns `image`/`gif`/`rayon`):

| crate | what |
|---|---|
| `noise-core` | 3D value-noise, fBm, domain warp, Worley, colour/ramp math. Bottom of everything. The lattice kernels are four-lane (`lanes.rs`); see the SIMD gotcha below. |
| `dither-core` | Bayer ordered dither + level quantization. |
| `scene-core` | `Camera`, seeded `Rng`, `Tile` + `blit` alpha compositor. |
| `background-core` | **The** backdrop — dithered ground, optional seeded nebula (baked + cached), parallax star layers. |
| `planet-core` | **The** planet renderer — 26-type table, sphere shader, weather, rings, moons. |
| `sun-core` | The compact star tile (granulation + corona). |
| `wasm-abi` | `alloc`/`dealloc` + opaque-handle macros for the C ABI. |
| `render-io` | GIF/contact-sheet/poster helpers, and the parallel GIF encoder. The only crate that touches `image`. |

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
passing that rect back into `render_tile_into` / `render_star_tile_into`. Two
things follow, and both matter:

- An **empty rect is the visibility test** — exact, and free. Do not add a
  "body radius × some margin" off-screen check next to it. There was one; it had
  to over-estimate for ringed giants and corona halos, so it kept rendering
  full-price tiles for bodies that were entirely off-screen.
- Pixels **outside the rect are not shaded**, so a tile is only valid for the
  placement its clip came from. Anything that caches a tile must put the clip in
  the cache key (`SunCache` does) — and then snap the rect outward to a grid
  (`snap_out`), or a camera drifting a pixel a frame invalidates the cache every
  frame and the caching buys nothing.

The rect is **exact**, not padded — `blit` reads tile pixel `map(dd)` for each
destination offset it visits and `map` is monotone, so the two endpoints bound
the set. Both functions share that expression for exactly that reason. Keep them
in step: a rect that under-reports by a pixel leaves an unshaded seam only
visible at the zoom levels nobody screenshots, which is what `scene-core`'s
million-read sweep is there to catch.

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
cargo run -q --release -p bird --bin alien   # `bird` has two bins; this one is easy to miss
(cd out && sha256sum *) | sort > /tmp/after.sha256
```

`out/` is gitignored and holds 74 files. Hash it **before** you touch anything,
again after, and diff. A refactor that is supposed to be behaviour-preserving
should come out byte-identical; if something changed, you must be able to name
which crate and why.

For the wasm side, check the C-ABI export set is unchanged — that is the contract
with the JS:

```bash
cargo build -p solar --target wasm32-unknown-unknown --release --no-default-features
node -e 'const m=new WebAssembly.Module(require("fs").readFileSync(process.argv[1]));
         console.log(WebAssembly.Module.exports(m).map(e=>e.kind+" "+e.name).sort().join("\n"))' \
     crates/solar/web/solar.wasm
```

Compare that list before/after. Changing it breaks a demo silently, because the
JS calls the exports by name.

Node also runs the modules directly, which is the only way to check the *wasm*
render path — `out/` only covers native. Instantiate with `{}` (there are no
imports), call `alloc`/`render`/`dealloc`, and hash the pixel bytes; that
checksum is to the wasm build what the `out/` hashes are to the native one, and
you need it whenever you touch `noise-core`.

Run wasm builds **from the repo root** so `.cargo/config.toml` applies. It adds
`-C target-feature=+simd128`, which is required, not optional — see below.

`cargo test --workspace` runs the handful of roster tests. It is fast; run it.

## Gotchas

- **The star's corona is tabulated, not shaded.** `sun_core::Shade` samples the
  halo's streamers, its falloff and the disc's limb darkening along one axis each,
  once per bake — the halo is ~65% of a star tile's pixels and none of those
  fields has per-pixel detail. The angular table is sized to the halo's outer
  circumference ×2.2 (`diamond_angle` covers a turn in 4 units but not at a
  constant rate — it is twice as steep at the diagonals). Shrink that factor and
  you get angular stair-steps in the halo that no still frame makes obvious; the
  crate's tests pin it against direct evaluation.
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
  For wasm, build the baseline worktree's module too and alternate `node` runs —
  and watch that the worktree build is what you think it is: cargo picks up
  `.cargo/config.toml` from the *working directory*, so a scratch crate built
  outside the repo silently loses `simd128` and will look like a regression.
- **`simd128` is load-bearing, not a bonus.** `noise-core`'s kernels are written
  four-lane. With the feature they beat the old scalar code; without it, the
  portable array fallback is *slower* in wasm than what it replaced. Never build
  a demo module with it off. Do not reach for `relaxed-simd` to go further: it
  permits FMA, whose rounding would split wasm output from the native
  generators'. `lanes.rs` documents the rules that keep the two backends
  bit-identical — read it before adding an operation there.
- **`render-io`'s `encode_gif` must stay byte-compatible with `image`.** It drives
  the `gif` crate directly — quantizing frames across cores with rayon, writing
  them serially — instead of using `image::codecs::gif::GifEncoder`, which is
  what makes the generators ~3.4× faster. It reproduces `GifEncoder`'s steps
  exactly (speed 1, `delay / 10`, `Background` disposal, empty global palette
  from the first frame, `set_repeat` first). If you touch it, or bump `image`
  or `gif`, re-run the `out/` hashes — every GIF in the repo depends on that
  correspondence, and a drift is invisible by eye.
- **Rendering order is parallel now; keep frames independent.** The generators
  run frames through rayon and `collect()` back into order, so output is
  deterministic — but only because every frame closure is a pure function of its
  index. A closure that accumulates across frames would silently produce garbage.
  The scene bins are the exception and stay serial: their `System`/`Belt`/`Scene`
  holds `RefCell` caches and is not `Sync` on purpose.
- **The sphere map is the sprite idea that works.** A screen-space sprite strip
  has to be re-baked for every (spin, light) pair and scales as r³ — at r=80 that
  is 503 frames and 51 MB, and it breaks even only after a full revolution
  because building it means rendering one. Indexing by (longitude, y) instead
  makes the bake invariant to both, so one map serves every frame. If a new layer
  looks bakeable, check it for an `angle` term first: `Terrestrial`/`Cratered`
  have none. `Emissive` splits instead — its rock field bakes, its flow stays
  live. `Banded` needed its drift re-expressed as a longitude rotation before it
  would bake at all, which is a look change and so has its own bit.
- **An axis that cannot be a lookup offset can still be a lookup *table*.** The
  cloud morph translates the noise domain in y and z — the field evolving — so no
  single map holds it and no offset fakes it. Six maps across the cycle, indexed
  by the morph value (it oscillates; do not index by time), recover 61% of what
  freezing cost for 4% of the frame. Memory and bake go up by the phase count,
  so the cache budget is sized for one zoomed planet's stack.
- **`F_ALL` is not every `F_*` bit.** It is the five that leave the picture
  alone. `F_NIGHT_LOD` was in it until `Lod` started feeding the aurora and the
  great spot as well as the base field — capping octaves past the terminator
  then moved pixels, and `out/` caught it. If a switch changes the image, it
  lives outside `F_ALL` and the web demos opt in.
- **A per-family optimization needs its gate checked one bit at a time.** The
  baked map was built only when `F_BAKED_CLOUDS` was set, so `F_BAKED_SURFACE`
  and `F_BAKED_BANDS` did nothing on their own — and the verification missed it
  for a whole commit because it always tested them with the cloud bit already on.
  Hash each bit against `F_ALL` alone, not against `F_ALL | <the other bits>`.
- **`F_BAKED_*` are deliberately outside `F_ALL`.** Every other `F_*` bit is
  either a feature the ablation panel switches off or an optimization that is
  invisible; these change the picture (frozen weather, baked albedo, thinned night side) to
  buy ~2x together on top of the shipped renderer.
  Keeping them out of `F_ALL` is what lets `out/` stay byte-identical while the
  web demos run with them. `System::frozen_clouds` / `MoonSystem::frozen_clouds` default to
  `false` for the same reason — the JS turns them on at construction.
- **A render-mode flag has to be in the tile memo key.** `solar` caches a
  planet's last tile against its geometry; a flag that changes how the tile is
  rendered but not where it lands will otherwise keep serving the old tile until
  the planet happens to move a pixel. That made the demo's checkboxes look dead
  on a slow world, and made an A/B measure the cache. `lod` and `frozen_clouds`
  are both folded in now.
- **Timing a scene means putting the body on screen and keeping the memo
  missing.** Three different ways to get this wrong, all of which produce a
  confident number: a camera parked where a planet started drifts off it within a
  few frames and then you are timing the backdrop (`solar`'s bench printed 0.28 ms
  for a scenario whose real cost is 40 ms); a camera that jumps far each frame
  re-bakes the backdrop and buries the body under it; and a time step too small to
  move the tile past its memo key means the shader never runs at all. Use
  `ms_follow` in `solar`'s bench as the template.
- **Vector code is not automatically faster inlined.** `value_noise` runs ~28×
  per pixel and had to be `#[inline(never)]` *on the vector path only* to stop
  its `v128` temporaries spilling in the pixel loop — inlined it was slower than
  scalar. Measure whole frames, not just the kernel: this one reversed sign
  between a microbenchmark and a real frame.
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
