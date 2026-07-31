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

**The GLSL ports are the sanctioned second shaders.** Four `.glsl` files
re-implement pixel loops for WebGL2 — `noise-core/src/noise.glsl` (the prelude
every one of them is concatenated after, carrying `#version` and the lattice
kernels), plus `planet-core`'s, `background-core`'s and `sun-core`'s bodies.
`dither-core/src/dither.glsl` rides along in the prelude. They earn the exception
by keeping the duplication to the shading: each crate's `gl_uniforms()` computes
its tables, seeded constants and octave budgets in Rust and ships them as one
float array, so `TYPES`/`SUNS`/`STAR_LAYERS` are *transported*, not copied — a
new planet type still means one row. Two consequences:

- The `U_*` slot indices in the GLSL and the `GL_U_*` constants in Rust are a
  wire format. `glsl_slot_indices_match_the_rust` parses the `#define`s and pins
  them, because a slot off by one paints a planet with somebody else's colours
  rather than failing. **It pins 16 of the 66 `#define`s.** The `O_*` octave
  slots are unpinned and are equally a wire format — `gl_octaves` writes them
  positionally — so renumbering one hands the aurora the crater's octave count,
  silently. Same for `S_BLOTCH_OCT`/`S_CORONA_OCT`. Pin a slot when you add one.
- `scripts/verify-gl.mjs` is to the GL path what `out/` is to the native one.
  Run it after touching any shader (`--demo all`). Expect a residue and read the
  right column: pixels differing by **more than one quantization level** are the
  signal (0.00% today); pixels differing by exactly one are ANGLE rounding a
  `sin` differently and landing across a `quant` threshold. `solar`'s raw differ
  rate is 17–30% and that is *fine*: it is the nebula, which the GPU evaluates
  per pixel where the CPU bakes it once per 8×8 cell and scrolls the sprite. At a
  zoom where the clouds fade the backdrop is byte-exact, which is how you know.
- **It needs `npm i -g playwright`, and nothing runs it for you** — no CI, no
  `package.json`. The 830 lines of GLSL are a second implementation whose only
  check is this script, so skipping it means shipping unverified. Its pass gate
  is also a *rate* (0.5%), so a handful of pixels can disagree by a lot and still
  pass; it warns when that rate is non-zero, so read the warning.

The GL path runs `F_ALL`, deliberately without `F_NIGHT_LOD`: that switch buys a
CPU back octaves it cannot afford and moves pixels on the dark limb doing it, and
a rasterizer does not need the trade. Leaving it off is what lets `verify-gl.mjs`
compare the two renderers pixel for pixel instead of approximately.

**A GPU scene is a draw list, not pixels.** `solar::gl_bodies` emits one record
per body — the destination rect from `dest_rect`, then that shader's uniform
block — sorted back-to-front, and the JS draws a quad each with alpha blending.
That *is* what `blit` was doing by hand. Two things to keep in step:

- The fragment shader maps its destination pixel back through the **same
  expression** `blit` uses (`int((dd + 0.5) / scale)`), which is what keeps
  `planet_pixel`/`sun_pixel` and the detail caps meaningful with no second render
  target. Change one, change both.
- The GPU has no `BackdropCache`, no `SunCache` and no `visible_tile_rect` — all
  three are caches for work a rasterizer does not mind repeating, and dropping
  them is most of the win. Do not port them back without a measurement.

**Scatter, don't gather.** `paint_stars` walks lit cells and plots one pixel
each. The first backdrop shader inverted that — every pixel testing nine cells in
each of three layers, 27 hashes per pixel against roughly one per fifty — and it
was three quarters of the fragment cost (216.6 ms/frame, vs 50.0 with the stars
as point sprites). `visit_stars` is now the one walk, feeding `paint_stars` and
`gl_star_points` alike. Before writing a gather into any shader, check whether
the vertex path will do. The nebula is the same shape of problem at 64x rather
than 1000x, and is the next candidate if the backdrop ever bites.

**The `gl` cargo feature is load-bearing, not tidiness.** Each core crate's
`mod gl` is `#[cfg(any(feature = "gl", test))]`, and the demo crates switch it on
only through
`[target.'cfg(target_arch = "wasm32")'.dependencies]` (plus `[dev-dependencies]`,
so `cargo test` still covers it — resolver 2 keeps those out of `cargo build`).
The reason is the codegen gotcha below: the native generators must not compile
this code at all, or `out/` moves.

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

**Web name arrays are hand-synced, in four places.** The C ABI can't return
strings, so `solar`'s roster is mirrored in `web/index.html`, `web/verify.mjs`
**and** `scripts/verify-gl.mjs`, and `moon`'s `PARENT_NAMES` in its HTML. Nothing
checks the lengths agree. Reorder a roster, `grep` for the array by name.

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

If you touched the planet shader or `shader.glsl`, diff the GPU path too — it is
a second implementation and nothing below the pixels checks it:

```bash
node scripts/verify-gl.mjs --types all      # needs the rebuilt planet.wasm
```

`cargo test --workspace` runs the roster tests and the GLSL wire-format checks.
It is fast; run it.

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
- **...and you do not have to *move* code to trigger it — adding some is enough.**
  `gl_uniforms` computes no pixels; it just reads the type row and calls
  `Lod::oct`, `moon_ring`, `seed_offsets`. Merely *existing* in `planet-core` as
  another caller of those re-priced their inlining and moved `out/moon_*.png` by
  up to 4/255 across 5% of its pixels — while `planet`, `solar`, `comet`,
  `asteroid` and `star` all stayed byte-identical, which is what makes this so
  easy to miss if you spot-check one crate. `mod gl` is therefore gated: the core
  crates use `#[cfg(any(feature = "gl", test))]` (they build for native too, as
  deps of the generators, so a plain `target_arch` test would not exclude them),
  and `solar` — itself a demo crate — uses
  `#[cfg(any(target_arch = "wasm32", test))]`. Either way the native generators do
  not carry the code and `out/` is byte-identical by construction. Note the
  bisection that found it — reverting the *refactors* changed nothing; only
  removing the new code did.
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
- **`F_ALL` is not every `F_*` bit.** It is the five that leave the picture
  alone. `F_NIGHT_LOD` was in it until `Lod` started feeding the aurora and the
  great spot as well as the base field — capping octaves past the terminator
  then moved pixels, and `out/` caught it. If a switch changes the image, it
  lives outside `F_ALL`, the callers that want it opt in (`System::night_lod`,
  `MoonSystem::night_lod`, both `false` by default so the generators do not get
  it), and `out/` stays byte-identical.
- **Timing a scene means putting the body on screen.** Two ways to get this
  wrong, both of which produce a confident number: a camera parked where a planet
  started drifts off it within a few frames and then you are timing the backdrop
  (`solar`'s bench printed 0.28 ms for a scenario whose real cost is 40 ms), and a
  camera that jumps far each frame re-bakes the nebula and buries the body under
  it. Use `ms_follow` in `solar`'s bench as the template. The star tile IS still
  memoized (`SunCache`), so a time step too small to cross `SUN_TQUANT` means its
  shader never runs.
- **Vector code is not automatically faster inlined.** `value_noise` runs ~28×
  per pixel and had to be `#[inline(never)]` *on the vector path only* to stop
  its `v128` temporaries spilling in the pixel loop — inlined it was slower than
  scalar. Measure whole frames, not just the kernel: this one reversed sign
  between a microbenchmark and a real frame.
- `cargo build --workspace` warns about a `bench` output-filename collision between
  `planet` and `solar`. Pre-existing; ignore it.
- **The browser build is single-threaded, and that is a choice, not a limit.** A
  wasm *instance* is one thread, but nothing stops N instances in N workers, and
  worker-per-region needs no COOP/COEP (unlike `SharedArrayBuffer`, which
  GitHub Pages can never provide). `scripts/make-parallel-probe.sh` measures
  what the cores are worth on a given host before anyone writes the pool —
  including whether its CSP allows the blob-URL workers a single-file build
  needs. 2.9x on 4 shared cores here.
- **...but a pool only pays if the workers render standalone.** `planet`'s does
  (2.7x on 4 cores: 4.57 -> 1.70 ms/frame). A *scene* pool was written and
  rejected: shipping each band's backdrop rows into a worker and the finished
  strip back out is ~4.3 MB of copying a frame at 900x600, which swamps the ~2 ms
  of body shading the split saves — 1.13x at best, and worse than no pool with an
  empty sky. Fixing it properly means threading a row range through
  `paint_backdrop`, `paint_stars` and `paint_orbit` so each worker paints its own
  band, and the GPU path deletes the whole problem instead. Measure the copy
  before writing the pool.
- **Amdahl lives in the backdrop.** It is full-frame serial work that scales with
  window area, and `bg_key` holds the camera — so a camera FOLLOWING a planet
  invalidates the cache every frame and repaints it, in every CPU path, pooled or
  not. That is why "16 workers changed nothing", and it is most of what moving to
  the GPU actually bought.
- **The GPU is only checkable, not measurable, in this sandbox.** There is no
  `/dev/dri`, so headless Chromium's WebGL2 is ANGLE over SwiftShader — a CPU
  rasterizer. `verify-gl.mjs` is a correctness harness and nothing else; a
  timing taken through it is a timing of the CPU. Say so rather than quoting it.
- **KNOWN DIVERGENCE: the orbit dots.** `paint_orbit` adds to a *ceiling*
  (26→90, 30→96, 40→120), so a dot crossing a bright star DARKENS it; the GPU's
  `blendFunc(ONE, ONE)` saturates at 255 instead. 62–1025 px/frame hit that
  ceiling at seeds 7/21 — real, ~0.04% of pixels, under `verify-gl`'s rate gate.
  Unfixed. Reproduce by counting pixels at exactly `(90, 96, 120)`.
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
