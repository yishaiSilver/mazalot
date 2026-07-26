# mazalot

Procedural, seed-driven pixel-art planets in Rust — **zero art assets**. Every
planet is generated from math per pixel, so a single seed always rebuilds the
exact same world. The core algorithm compiles to both a native GIF/PNG generator
and a ~56 KB WebAssembly module from **one shared codebase**.

There's also a companion **star** generator (a sibling of the planet renderer),
a draggable **solar-system** view that composes a star with orbiting planets, a
paper-doll **character** compositor, and a fully separate **creature** generator
(alien + earth birds) — see below.

## Crate layout

A Cargo workspace under `crates/`. Each **demo crate** has the same three faces —
`lib.rs` (pure render math), `wasm.rs` (a raw C-ABI cdylib face, **no
wasm-bindgen**), and `src/bin/*` (native GIF/PNG generators behind a `native`
feature) — and they share their common machinery through **library crates**
rather than copy-pasting it. The library crates carry **no third-party deps**, so
a `--no-default-features` wasm build never sees `image`/`rand` and stays tiny.

**Library crates (shared, dependency-free):**

| Crate | What it is |
|-------|------------|
| `noise-core` | 3D value-noise + fBm + domain warp + Worley, and the color/ramp math. The bottom of everything. The two lattice kernels hash four corners per instruction on wasm `simd128` — see [SIMD noise](#simd-noise). |
| `dither-core` | Bayer ordered dithering and level quantization — the pixel-art output stage. |
| `scene-core` | The scene-compositor kit: draggable `Camera`, seeded `Rng`, and the `Tile` + `blit` alpha compositor. |
| `background-core` | Everything a scene paints *before* its bodies: the dithered navy ground, an optional seeded **nebula** (baked at low res into a world-indexed sprite that a pan scrolls rather than rebuilds), and **parallax star layers**. |
| `planet-core` | **The** planet renderer — the only one in the workspace. The 26-type table, sphere shading, weather, rings, moons. One shader, two framings: a *hero* square frame (`render_rgba`) and a *scene sprite tile* (`render_tile`). `planet`, `solar` and `moon` are all framings of it. |
| `sun-core` | The compact star tile (granulation + corona) used by `solar` and `comet`. |
| `wasm-abi` | The raw C-ABI glue: `alloc`/`dealloc` and opaque-handle macros. Exports no symbols itself. |
| `render-io` | The only crate that touches `image`: GIF/contact-sheet/poster helpers for the native bins, plus the parallel GIF encoder that makes them fast — see [Parallel generation](#parallel-generation). |

**Demo crates:**

| Crate | What it is |
|-------|------------|
| `planet` | One planet filling the frame — 26 types, full tuning controls in the web demo. |
| `star` | One star filling the frame: granulation, sunspots, prominences, corona. |
| `solar` | A draggable, zoomable **solar system** — a star with `planet-core`'s worlds in eccentric orbit around it. |
| `moon` | A `planet-core` world with depth-sorted moons orbiting it. |
| `asteroid` | Drifting, perspective-squashed asteroid belts. |
| `comet` | Eccentric-orbit comets with anti-sunward tails. |
| `character` | A paper-doll character compositor (native only). |
| `bird` | A fully separate creature generator: `--bin alien` (hybrid alien "genus" families) and `--bin bird` (naturalistic earth birds). Shares nothing with the planet crates. |

### Who imports what

● declared in that crate's `Cargo.toml` · ○ pulled in transitively · `render-io` is
always behind the `native` feature, so the wasm build never sees it.

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

`planet` is 18 lines because it is a face over `planet-core` — the same rlib `solar`
and `moon` render their bodies with. Note that they depend on the **rlib**, not on
the `planet` crate: `planet` is a cdylib whose `#[no_mangle]` exports (`render`,
`alloc`, `dealloc`) would collide with each scene crate's own in the wasm build.
Demo crates depend on library crates, never on each other.

## The planet system

**26 types** across **5 base algorithms** — terrestrial (terran, ocean,
archipelago, desert, swamp, iron, ice, savanna, gaia, tundra, alpine, obsidian,
chrome), cratered (barren, moon), banded gas/ice/storm/ringed giants, emissive
(lava, molten sea, radioactive, fungal, crystal), and cloud-shrouded (toxic,
storm shroud) — plus **rings**, **orbiting moons**, and material-aware
**specular glare**.

### Fake 3D
For each pixel of the disc, treat it as the front hemisphere of a unit sphere,
rotate the surface point around Y by the spin angle, and sample **3D noise**
there. Shade with a fixed light (Lambert + Blinn-Phong specular scaled by local
albedo) and an atmosphere rim. Sampling in 3D means no seams and no pole
stretching, and a full 360° spin loops seamlessly.

### Animated weather (loop-safe)
- **Clouds** — domain-warped wispy fronts that drift and billow; cast soft shadows.
- **Gas-giant bands** — counter-rotating zonal jets + domain warp (fluid, not a sine wobble).
- **Great spot** — a drifting spiral cyclone with a calm eye.
- **Lightning** — small, irregular, randomized-color flashes on storm worlds.
- **Aurorae** — shimmering polar curtains, hue palette-cycled green→cyan→violet.
- **Storm cells** — bounded hurricane swirls in the cloud layer.
- **Molten flow** — palette-cycled glow that flows across lava/emissive worlds.

### Pixel-art output
- **Ordered (Bayer) dithering** — kills ramp banding, dithers the terminator, stays crisp under spin.
- **Limited palettes** — swap any planet into a duotone: `Natural`, `Game Boy`, `Ice`, `Sunset`.
- **Crisp dark rim** — a 1-px outline on every disc (and every moon) for sprite readability.

## The star system

A star is the **inverse of a planet**: self-luminous, so there is *no* day/night
terminator and no external light — the whole disc glows. The `star` crate reuses
the shared `noise-core`/`dither-core` helpers and adds star-specific shading:

- **Granulation** — Worley convection cells (bright centres, dark inter-granular lanes) plus a warped-fbm mottle, boiling over time (loop-safe).
- **Sunspots** — low-frequency umbrae that drift slowly across the surface.
- **Limb darkening** — the edge dims and tints cooler (`mu = nz`), which is what gives the flat disc its spherical read.
- **Corona** — a soft halo with shimmering radial streamers past the limb.
- **Prominences** — jagged filaments erupting from evenly-spaced limb lobes, each firing on its own seamless pulse; flare stars add rare violent spikes.
- **Sparkle motes** — twinkling points in the halo.

**8 types** across the temperature spectrum — `blue_giant`, `white_star`,
`yellow_dwarf`, `orange_dwarf`, `red_giant`, `red_dwarf`, `white_dwarf` — plus an
exotic teal `sol` (a nod to *rebels-in-the-sky*). Add a star type = add one row
to `STYPES` in `crates/star/src/lib.rs`.

## The solar system

Where `planet` and `star` each render *one* body filling a square, `solar`
(`crates/solar`) renders a **whole system** into an arbitrary rectangular
viewport that you can **drag around** and **zoom into** — a central star with
planets orbiting it, against a starfield. Same seed => the same system, forever.

The worlds in orbit here are **literally the `planet` demo's worlds** — same
archetype table, same shader — asked for in `planet-core`'s *sprite* framing: a
transparent tile, sized to its disc and lit from an arbitrary direction, instead
of a hero planet filling a square. The star is `sun-core`'s compact tile. So the
new work here is the layer on top:

- **Orbital layout** — from a seed: a star (one of 5 archetypes), then 4–8
  planets placed outward in bands, so rocky/lava worlds fall near the star and
  gas/ice giants far out. Speeds are Keplerian-ish (inner planets sweep faster).
- **Sun-lit phases** — each planet is lit from the star's *screen* direction, so
  worlds show crescent → gibbous phases depending on where they are in orbit.
- **Depth sorting** — planets are drawn back-to-front by orbital depth, so one on
  the far side is occluded by the sun and one on the near side passes in front.
- **Draggable camera** — a world→screen camera; drag to pan, zoom about the
  viewport centre (keeps the scene + parallax anchored no matter where you've
  panned).
- **Space background** — `background-core`, shared with moon/comet/asteroid: a
  faint seed-colored **nebula** (baked at low res → pixel-art clouds) plus three
  **parallax** star layers with temperature
  colors. Each layer is a fixed *screen-space* grid scrolled by the camera's
  **accumulated screen-space pan** (Δcam·zoom summed over time) at a fraction `p`
  of the foreground — so on **pan** the stars always move slower than the system
  by the same ratio at every zoom (no runaway when zoomed out), and on **zoom**
  they don't move at all (pure zoom adds no screen displacement, and zoom is
  about the viewport centre). So a star can never move faster than the solar
  system, and the on-screen count stays constant (no wall, no swim). **Star
  density** and **star parallax** controls tune the count and pan scroll rate. Stars are 1px points plotted by iterating the
  visible grid cells — O(cells), not O(pixels). The far layer and the nebula fade
  out (and are skipped) when you zoom in on a body. The backdrop depends only on
  the camera + view params (never on animation time), so it's **cached**: on a
  still camera — the common "watch it orbit" view — the whole background is a
  `memcpy` and only the bodies re-render, and a drag scrolls the cached backdrop
  and repaints only the edge that came into view. This is why the fit view runs at ~110
  fps native while orbiting (see Performance).
- **Click to follow** — click a planet and the camera locks on and tracks it
  around its orbit; drag anywhere to release.

Each frame: paint the background → dot in each orbit path → render every body to
a small RGBA tile and alpha-blend it in, depth-sorted. Bodies are small, so the
whole scene stays cheap enough to render live *while you drag*.

**Add a world to the roster** = add a row to `ROSTER` in `crates/solar/src/lib.rs`
naming a `planet-core` type and the orbital band it belongs in (the archetype
itself is defined once, over in `planet-core`); **add a star** = add a row to
`SUNS` in the same file.

## Running it

**Native — generate GIFs + PNG into `out/`:**
```bash
cargo run --release --bin planet            # planets
cargo run --release --bin sun               # stars
cargo run --release -p solar --bin solar    # solar systems (orbit + pan GIFs, posters)
cargo run --release --bin sprite-compositor # characters
cargo run --release --bin bench             # feature-cost benchmark
cargo run --release -p bird --bin alien     # alien creatures  (disjoint half)
cargo run --release -p bird --bin bird      # earth birds       (disjoint half)
```

**Web — live, interactive planet:**
```bash
cargo build -p planet-web --target wasm32-unknown-unknown --release
cp target/wasm32-unknown-unknown/release/planet_web.wasm web/planet.wasm
cd web && python3 -m http.server 8000       # open http://localhost:8000/
```

**Web — live, interactive star:**
```bash
cargo build -p star --target wasm32-unknown-unknown --release --no-default-features
cp target/wasm32-unknown-unknown/release/star.wasm crates/star/web/star.wasm
cd crates/star/web && python3 -m http.server 8000   # open http://localhost:8000/
```

**Web — live, draggable solar system:**
```bash
cargo build -p solar --target wasm32-unknown-unknown --release --no-default-features
cp target/wasm32-unknown-unknown/release/solar.wasm crates/solar/web/solar.wasm
cd crates/solar/web && python3 -m http.server 8000   # open http://localhost:8000/
```
Drag to pan · scroll / pinch to zoom · tap a planet to follow it. Zoom reveals
detail rather than magnifying fixed pixels — the render buffer is sized so a
rendered pixel is a constant on-screen block at every zoom, while bodies are
rendered at a resolution that grows as you zoom in. A **Controls** dock exposes
manual overrides:
- **Layout** — planet count, planet spacing, planet size, sun size, **orbit
  thickness** (dashed-path line weight), and **eccentricity** (0 = circular
  orbits, 1 = as generated, up to 2 = exaggerated ellipses).
- **Motion** — orbit speed, and separate **planet** and **star rotation** speeds
  (three independent clocks; each accumulates so changing a speed never jumps).
- **Pixelation** — scene / planet / sun pixel size, plus per-body **detail caps**
  (planets and sun separately): the "lower bound of pixelation" — how far you can
  zoom before a body stops resolving finer and just enlarges its blocks. Lower
  caps also keep zoomed-in views cheap.
- **Background** — **star density** (how many background stars, constant across
  zoom) and **star parallax** (scroll-rate multiplier: 0 pins the stars on pan,
  higher makes them scroll faster / feel closer).

A **performance readout** (top-right) shows live, smoothed **FPS**, the **WASM
render time** (the procedural CPU cost per frame), the whole-frame time, that
render as a **percent of a 60 fps CPU budget** (with a green→amber→red bar), the
current render resolution, and whether the **backdrop was cached or redrawn**
this frame — so you can watch it flip to "cached ✓" the instant you stop dragging
(see Performance). Click it to collapse to the one-line summary; press **P** to
hide it.

Sizes/spacing/pixelation/detail-caps are live view params applied to the system
(`system_set_view`) with no regeneration; only seed and planet count rebuild it.
Off-screen bodies are culled and each body's tile is bounded, so zoom stays
responsive. Works on touch/mobile. (`node verify.mjs` renders the system
headlessly as a build check.)

**Web — the solar-system companion demos (moons, asteroid belt, comet):**
```bash
for c in moon asteroid comet; do
  cargo build -p $c --target wasm32-unknown-unknown --release --no-default-features
  cp target/wasm32-unknown-unknown/release/$c.wasm crates/$c/web/$c.wasm
done
python3 -m http.server 8000   # open http://localhost:8000/ and pick a demo
```
Each is a sibling of the solar demo — drag to pan, scroll / pinch to zoom, a
collapsible **Controls** dock, and the same constant-block pixel-art scheme:
- **moon** — a planet with orbiting moons, depth-sorted so they pass in front of
  and behind it. Sliders: moon count, orbit speed, scene pixelation.
- **asteroid** — a drifting belt; live `belt_set_view` sliders for rock count,
  spacing, rock size, star density, and a center-marker toggle.
- **comet** — a comet on an eccentric orbit with an anti-sunward tail; a **Follow
  comet** button locks the camera to the head as it whips through perihelion.

Eccentric orbits themselves live in the **solar** demo: each planet travels a
Kepler ellipse with the star at a focus (solved from the mean anomaly, so worlds
speed up at perihelion), and an **Eccentricity** slider scales the whole system
from perfectly circular to exaggerated.

**Bundling a demo into one file.** To turn any demo into a single self-contained
HTML with its wasm inlined (runs with no server — open it locally, host it
anywhere, or publish it as a Claude artifact), use:

```bash
scripts/make-artifact.sh solar    # -> dist/solar.html
```

See [docs/artifacts.md](docs/artifacts.md) for options and details.

**Web — live creature (the bird half):**
```bash
cargo build -p bird-web --target wasm32-unknown-unknown --release
cp target/wasm32-unknown-unknown/release/bird_web.wasm bird-web/bird.wasm
cd bird-web && python3 -m http.server 8000  # open http://localhost:8000/
```
(All require the wasm target: `rustup target add wasm32-unknown-unknown`. The
`--no-default-features` flag drops the native-only `image`/`rand` deps so the
wasm build stays tiny. `simd128` comes from `.cargo/config.toml` — run these from
the repo root so cargo picks it up; see [SIMD noise](#simd-noise).)

### Web controls
Type · Seed · Resolution · Spin, then live sliders for every parameter
(contrast, frequency, ice caps, specular, shininess, glare↔albedo, clouds,
storm cells, aurora, lightning, great spot, bands, turbulence) plus a **Look**
section — palette swap, dither, orbiting-moons toggle, and a CRT/scanline toggle.
Sliders snap to each type's defaults on selection.

## Performance

Rendering is **per-pixel procedural**: every frame recomputes noise for every
pixel. A sprite is a `memcpy`; a planet is thousands of times more expensive.
Measured natively (WASM in-browser runs ~2–3× slower):

| @ 64px | per frame | vs a sprite |
|---|---|---|
| sprite blit (`memcpy`) | ~0.0003 ms | 1× |
| planet, no weather (iron) | 0.67 ms | ~3,200× |
| planet, full weather (terran) | 1.98 ms | ~9,400× |
| lava (emissive) | 0.79 ms | ~3,800× |

**The weather is the cost** — domain warp on clouds/bands roughly triples the
base planet. **The pixel-art pipeline is nearly free:** dithering, moons, and
palette swaps together add **< 0.05 ms** (a few percent).

Implications:
- **One planet live** (the web demo): comfortable — ~2 ms native, ~5–7 ms in WASM at 64 px, well under a 60 fps budget. Tightens above ~200 px.
- **Many planets / a galaxy map**: don't render live. **Bake the ~30 spin frames once, then blit** (that ~0.0003 ms) — procedural variety at sprite-cheap playback.
- **Cheaper weather:** dropping domain warp (back to plain fBm) roughly halves the weather cost.

### Profiling the solar system

`cargo run -p solar --release --bin bench` decomposes a frame by rendering the
same scene under controlled scenarios (bodies culled off-screen → background
only; density 0 → no stars; zoomed past the nebula fade → base fill only).
At 1000×640, seed 7 (4 planets), native, **after the caching below**:

| scenario | before caching | after |
|---|---|---|
| fit view, panning (drag) | ~17 ms · 58 fps | **~4 ms · 240 fps** |
| fit view, still camera | ~17 ms · 58 fps | **~2.6 ms · 385 fps** |
| zoomed onto the sun | ~39 ms · 26 fps | **~8 ms · 125 fps** |
| zoomed onto a planet | ~35 ms · 29 fps | ~34 ms · 30 fps |

Everything expensive here is **time-quantized cached**: the costly input evolves
slowly, so it's snapped to a coarse step and reused between re-bakes. The same
trick is applied at three scales.

**Background** — profiling the uncached renderer showed it was ~50% of every
frame and O(pixels), yet almost entirely *stable*: it never depends on animation
time, the nebula scrolls at only 9% of pan and its shape is zoom-independent, and
the base navy is constant. Only the stars (a cheap O(cells) overlay) truly move.

Time-quantizing alone only got a *still* camera cheap, though — the moment you
dragged, every key changed and the whole backdrop was rebuilt. The fix is to stop
treating a pan as invalidation: the backdrop is a function of *where* the camera
is, so a panned frame is the previous frame **moved**. Both cached layers are
therefore kept as **sprites indexed by world position**, and a drag memmoves them
and repaints only the strip that scrolled into view:
- **fBm nebula field** (low-res) — indexed by absolute world cell. A pan slides
  it and re-bakes ~1% of it; a pure **zoom never re-bakes it at all**, because
  the fade is applied when the sprite is read rather than baked in.
- **Base-navy + nebula layer** (full-res, all but stars/orbits) — the dominant
  cost of the two, by about 4:1. Scrolled the same way, so a drag costs a sliver
  of a screenful instead of a full composite.
- **Whole backdrop** (`render_system_cached`) — a still camera skips even the
  star overlay: the entire backdrop is one `memcpy`, only bodies re-render.

Measured at 1000x640 (`cargo run --release -p solar --bin bench`), a fast drag
went 4.9 ms → 0.33 ms, and the backdrop from 45% of a panning frame to 22%.

What makes the sprites slide is that the whole layer is a function of where the
*clouds* are — so the nebula's ordered dither is anchored to them rather than to
the screen (which also stops the stipple crawling as they drift). A scene that
sets `Backdrop::dither` under a nebula reintroduces a screen-pinned term and
opts out of the scrolled path; none currently does.

The first two layers live in `background_core::BackdropCache`, so any scene that
grows a nebula inherits them; the third is solar's, since it also caches the
orbit paths. `cargo test -p background-core` pins the fast paths to the uncached
renderer byte-for-byte across a scripted pan — a scroll bug shows up as stale sky
smeared across a drag and in no still frame, so it needs a test that pans.

**Bodies** — with the background cached, bodies dominated, and the star's
convection/corona shader (27-cell worley + fBm per pixel over a large tile) was
the single worst case at ~39 ms. But the boil evolves slowly, so the **star tile
is cached** exactly like the nebula: keyed on the render radius + a quantized
boil clock (`SUN_TQUANT`), it's re-baked every few frames instead of every frame
(**sun 39 → ~8 ms**), and a still or non-rotating star is essentially free. At
extreme zoom the tile also drops its secondary-fBm octaves (below the dither
floor at that size) for a cheaper re-bake. The per-frame draw-order `Vec` and the
star tile's 577 KB alloc are both gone (reused/cached).

Everything invalidates automatically the moment its key changes. **Remaining
frontier:** a *planet* zoomed to fill the screen (~34 ms at a high detail cap).
Planets aren't tile-cached because their axial rotation is the visible motion, so
quantizing it (the sun trick) would read as choppy.

The cost scales with the **detail cap** — it bounds the tile resolution, and the
per-pixel shader runs once per tile pixel. Two ways to keep it cheap:

- **Let it pin itself.** The demo holds a render budget by walking the planet
  detail cap down when a frame runs long and back up when it doesn't (*Hold 60
  fps automatically*, on by default; the slider becomes the ceiling and the HUD
  shows the effective value). Body tile resolution is by far the steepest knob
  in the scene — following a planet at 20× zoom costs **53 ms at cap 160 and
  10 ms at cap 64** — and it degrades the way pixel art wants to, by getting
  chunkier rather than blurrier. A tile costs ~r², so the controller jumps
  straight to `cap · sqrt(budget / measured)` instead of walking: it converges
  in two ticks (~0.4 s) and settles just under budget.

  Worth knowing what it is *not*: at that zoom the backdrop cache is worth only
  ~0.5 ms whether it hits or misses, turning the starfield and nebula off
  entirely changes nothing, and the sun's cap is irrelevant because the star is
  culled once you are on a planet. It is the planet tile, all of it.

- **Pin the detail cap low** (~56) — the tile stays small, so the fills-screen
  case never gets expensive in the first place. This is the intended default and
  needs no code: the cap is already a live slider (`planet_detail`). At ~56 a
  full-screen planet is **~6 ms (170 fps)** instead of ~34 ms — the `bench` bin
  measures both.
- **Octave LOD** — implemented, at `sun-core`'s threshold (`size > 200`) and
  behind a toggle in the demo. Worth **9–19%** on a zoomed-in planet for a mean
  change of 0.8–4.5/255 across the disc; one 1px step of spin already moves ~40%
  of it with a mean near 10, so the LOD sits well inside the frame-to-frame noise
  the eye accepts. Below 200px it is bit-identical, so `out/` never sees it —
  solar's biggest native planet tile is r≈12, moon's r≈85.

  The tuning is the opposite of what it looks like. Clouds are 61% of a `terran`
  frame, so cutting them is where the speed is — but one dropped *cloud* octave
  moves 22% of the disc (mean 3.3) against 3% (mean 1.3) for one dropped
  *surface* octave. The broad low-contrast layer is what the eye reads as
  silhouette, so the surface octave goes first and clouds only follow past 400px.

  Keep it in proportion: 9–19% is barely above this machine's timing noise, and
  pinning `planet_detail` low is still the 5–6× lever.

### SIMD noise

Everything above is about doing the shader *less often*. This is about making it
cheaper: `value_noise` and `worley` hash their lattice corners four at a time
through a small four-lane shim (`crates/noise-core/src/lanes.rs`), which lowers
to wasm `simd128` in the browser and to plain `[_; 4]` arrays everywhere else.
Measured in V8 (64²/128² frames, ms):

| | before | after |
|---|---|---|
| `terran` (clouds + aurora + storm) | 3.46 / 13.32 | **3.10 / 12.17** |
| `gas_giant` (warped bands + spot)  | 3.13 / 12.32 | **2.71 / 11.00** |
| `barren` (worley craters)          | 1.40 / 5.65  | **1.11 / 4.56** |
| `sun-core` tile, r = 24 / 60 / 140 px | 2.88 / 15.6 / 85.5 | **2.40 / 13.1 / 71.0** |
| `solar` scene frame @ 960×540      | 7.4          | **6.4** |

Every rendered byte is unchanged — native `out/` hashes and the wasm pixel
checksums both match the scalar code exactly, which is the point: the shim is
written so the two backends cannot diverge (no FMA, no reassociation), and
`cargo test -p noise-core` pins both kernels to the scalar definitions they
replaced, bit-for-bit.

Two results worth keeping, because both are counter-intuitive:

- **`-C target-feature=+simd128` on its own does nothing.** LLVM will not
  auto-vectorize these kernels; the flag alone produced byte-identical output at
  identical speed. The lanes have to be written by hand.
- **The four-lane `value_noise` is only a win when it is *not* inlined.** A pixel
  evaluates it ~28 times, and inlined into the pixel loop its `v128` temporaries
  spill badly enough to end up *slower* than scalar (4.12 ms vs 3.46). Out of
  line it wins (3.10). `worley` is called once per pixel and wants ordinary
  inlining. Hence the `cfg_attr` on `value_noise`.

Without `simd128` the shim's portable path is slower **in wasm** than the scalar
code it replaced (`barren` 1.63 vs 1.40 ms), so the feature is a requirement of
the committed modules rather than a bonus — which is why it lives in
`.cargo/config.toml` and not in a build script. Native is unaffected either way
(it takes the array path, and measured within noise of before).

### Shader experiments

Five ideas were built and measured against each other; three earned their place.
Unlike everything above, these **change pixels** — the cost is stated for each.

| experiment | best case | visual cost | kept |
|---|---|---|:--:|
| **Cheap warp inner field** — `fbm_warp`'s three displacement fields only bend the outer field's domain, so they run at 2 octaves instead of matching it | 16–38% on cloudy/banded worlds | 12–30% of the disc moves, mean 1.4–4.2/255 | ✅ |
| **Night-side thinning** — past the terminator `shade` bottoms out at the 0.10 ambient floor, leaving ~3 of 22 levels, so the fine octaves and the whole cloud deck are skipped there | 8–35% | 3–7% of the disc, mean 0.03–0.9 | ✅ |
| **Tile bbox** — a ringed giant's tile is 4.4r across for a 2r disc; bound each row to its content | 9–14% on ringed worlds | none — bit-exact | ✅ |
| **Planet tile memo** — reuse a body's last tile while nothing it depends on has moved a whole pixel (the sun has done this for ages) | 16–35% at fit view, **65% paused when zoomed** | motion quantizes to ≤1px | ✅ |
| **Cubic interpolant** — replace the quintic `smoother` | 0% — the hash dominates and the lerps are already vectorized | — | ❌ |
| **Worley early-out** — skip cells whose bounding box is farther than the best hit so far | **3× slower**, despite skipping 19.4 of 27 cells exactly | none | ❌ |

That last one is the useful negative: branchless four-lane SIMD beats a smarter
scalar loop, and the per-call bookkeeping to decide what to skip costs more than
the work it saves. Don't add branches to reclaim work from a vectorized kernel.

For scale on the visual cost: one 1px step of axial spin already changes ~40% of
a disc with a mean delta near 10. Across the whole shipped `planets_table.png`,
all of this together moves **4.6% of pixels by a mean of 0.63/255**; the sun
table is untouched (the `star` crate doesn't share this shader).

Net: **13–20% off a wasm planet frame**, ~8% off a solar scene frame, and up to
65% when the scene is parked. The native generators barely move (~1%), which is
the encoder-bound result above showing up exactly where it should.

### Parallel generation

The browser work above does not help the native generators, because they were
never shader-bound. Profiling the whole pipeline found the shading to be **~9%**
of it; the other ~85% was **GIF encoding** — NeuQuant palette quantization at
`image`'s default `speed = 1`. A single file, planet's all-types grid GIF, was
19s of a 30s run by itself.

So `render-io` drives the `gif` crate directly instead of `image`'s `GifEncoder`.
The encoder is one stateful writer, but the quantization of each frame is a pure
function of that frame — so quantization fans out across cores with `rayon` and
only the writes stay ordered and serial. Frame *production* is parallel too for
the spinning-body family. On 4 cores:

| generator | before | after | |
|---|---:|---:|---:|
| `comet`    | 19.6s | **5.3s** | 3.68× |
| `planet`   | 31.2s | **8.7s** | 3.60× |
| `sun`      | 13.1s | **3.7s** | 3.54× |
| `solar`    | 17.6s | **5.1s** | 3.47× |
| `asteroid` | 15.3s | **4.5s** | 3.40× |
| `moon`     |  8.2s | **2.5s** | 3.32× |
| **all six** | **105s** | **30s** | **3.52×** |

Counting `bird` and `character`, which are untouched, the whole `out/` build goes
110.5s → 35.1s (3.15×).

Every byte is unchanged, which is the whole constraint: `encode_gif` mirrors
`GifEncoder::convert_frame`/`encode_gif` step for step — same speed, same
`delay / 10` truncation, same `Background` disposal, same empty global palette
sized from the first frame, `set_repeat` before any frame. It is the one place
in the workspace that has to stay byte-compatible with someone else's encoder,
so changes there get checked against the `out/` hashes, not by eye.

The scene generators (`solar`, `moon`, `comet`, `asteroid`) parallelize only
their encoding: their frame closures borrow a `System`/`Belt`/`Scene` whose
`RefCell` caches — draw order, the baked sun tile, the nebula — make it
deliberately non-`Sync`. Rendering frames in parallel would mean one system per
thread, which throws away exactly the caches that make a scene frame cheap. They
still land near 3.5× because encoding was the bulk of it.

`bird` is deliberately untouched. It renders creatures, not planets; it keeps its
own inline GIF encoder and its independence from `render-io`, and it stays on the
serial path. That is why the whole-pipeline figure lands below the per-generator
one — `alien` is ~5s of it.

Nothing here reaches the wasm builds: `render-io` sits behind the `native`
feature, so a `--no-default-features` module still has **zero** third-party
dependencies.

## Adding a planet type

Add one row to `TYPES` in `crates/planet-core/src/lib.rs` — palette, thresholds,
flags. The native GIFs and the web demo pick it up automatically; there is only
one copy of the algorithm. To put the new world into orbit as well, add it to
`ROSTER` in `crates/solar/src/lib.rs` (a name + which band it belongs in) —
`solar` renders it with the very same shader.
