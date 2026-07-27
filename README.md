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
| `noise-core` | 3D value-noise + fBm + domain warp + Worley, and the color/ramp math. The bottom of everything. |
| `dither-core` | Bayer ordered dithering and level quantization — the pixel-art output stage. |
| `scene-core` | The scene-compositor kit: draggable `Camera`, seeded `Rng`, and the `Tile` + `blit` alpha compositor. |
| `background-core` | Everything a scene paints *before* its bodies: the dithered navy ground, an optional seeded **nebula** (baked at low res into a world-indexed sprite that a pan scrolls rather than rebuilds), and **parallax star layers**. |
| `planet-core` | **The** planet renderer — the only one in the workspace. The 26-type table, sphere shading, weather, rings, moons. One shader, two framings: a *hero* square frame (`render_rgba`) and a *scene sprite tile* (`render_tile`). `planet`, `solar` and `moon` are all framings of it. |
| `sun-core` | The compact star tile (granulation + corona) used by `solar` and `comet`. |
| `wasm-abi` | The raw C-ABI glue: `alloc`/`dealloc` and opaque-handle macros. Exports no symbols itself. |
| `render-io` | The only crate that touches `image`: GIF/contact-sheet/poster helpers for the native bins. |

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
| `noise-core`       |   146 |   ○    |  ●   |   ●   |  ●   |   ●   |    ●     |
| `dither-core`      |    31 |   ○    |  ●   |   ○   |  ●   |   ●   |    ●     |
| `scene-core`       |   130 |   ○    |  ·   |   ●   |  ●   |   ●   |    ●     |
| `background-core`  |   365 |   ·    |  ·   |   ●   |  ●   |   ●   |    ●     |
| `planet-core`      |   815 |   ●    |  ·   |   ●   |  ●   |   ·   |    ·     |
| `sun-core`         |   124 |   ·    |  ·   |   ●   |  ·   |   ●   |    ·     |
| `wasm-abi`         |    87 |   ●    |  ●   |   ●   |  ●   |   ●   |    ●     |
| `render-io`        |   188 |   ●    |  ●   |   ●   |  ●   |   ●   |    ●     |
| **`lib.rs`**       |       | **18** | 567  |  767  | 503  |  557  |   490    |
| **`wasm.rs`**      |       |   93   |  58  |  166  |  76  |   79  |    71    |

The library layer is 8 crates / 1,886 lines and stacks in one direction only:

```
                          render-io ──── image (the only third-party dep)
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
wasm build stays tiny.)

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
| sprite blit (`memcpy`) | ~0.0002 ms | 1× |
| planet, no weather (iron) | 0.45 ms | ~2,500× |
| planet, full weather (terran) | 1.49 ms | ~8,400× |
| lava (emissive) | 0.64 ms | ~3,600× |

**The weather is the cost** — domain warp on clouds/bands roughly triples the
base planet. **The pixel-art pipeline is nearly free:** dithering, moons, and
palette swaps together add **< 0.05 ms** (a few percent).

Cost is **quadratic in the rendered size**, but not quite: the octave count is
tied to what the pixel grid can resolve (see `planet_core::Lod`), so a small
planet drops the octaves it could never have shown. A 64 px terran runs 3
octaves of surface noise where a 256 px one runs 6.

Implications:
- **One planet live** (the web demo): comfortable — ~1.5 ms native, ~4–5 ms in WASM at 64 px, well under a 60 fps budget. Tightens above ~200 px.
- **Many planets / a galaxy map**: don't render live. **Bake the ~30 spin frames once, then blit** (that ~0.0002 ms) — procedural variety at sprite-cheap playback.
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

Everything invalidates automatically the moment its key changes.

**Zoomed-in planets.** Planets can't be tile-cached the way the star is — their
axial rotation *is* the visible motion, and at any usable quantum the tile
changes every frame anyway. So the zoomed-in case is attacked by doing less work
per frame rather than by reusing frames, in three independent ways:

- **Shade only what the compositor will read.** `scene_core::visible_tile_rect`
  asks where a tile actually lands on screen and returns the sub-rect `blit` will
  sample; `planet_core::render_tile_clipped` shades that and nothing else. A disc
  twice the viewport height has ~70% of its tile hanging off the edge, and that
  work simply stops happening. An empty rect is also the exact visibility test,
  which replaced a blanket "2.2 body radii" cull margin that had to over-estimate
  for ringed giants — and so kept rendering full-size tiles for plain worlds that
  were already off-screen. Rows are further narrowed to the span the disc (and a
  ringed world's ring ellipse) actually covers.
- **Octaves finer than the pixel grid are dropped** (`planet_core::Lod`). A tile
  puts one sphere radius across `rad` px, so a field sampled at `p · freq` has
  its `k`-th octave land at `rad / (freq · 2^(k-1))` px. Under two pixels it is
  past Nyquist — it cannot be resolved, and since the planet turns, what it
  contributes is crawling speckle. This is a mip level, not a quality knob: a
  planet 20 px across pays for three octaves, the same planet filling the screen
  pays for six, and coastlines come out *steadier* than before. Separately, a
  domain warp's displacement field only needs two octaves whatever the size
  (`noise_core::fbm_warp_oct`), which alone turns a 4-octave `fbm_warp`'s 16
  octave evaluations into 10.
- **The compositor walks runs, not pixels.** At the upscales a zoomed-in scene
  reaches, tens of consecutive destination px share one tile px, so `blit` fetches
  the source, tests alpha and computes the blend factors once per run — and skips
  a transparent run without touching the destination.

Measured in the browser (V8, a 1680×944 render buffer — what the demo picks for a
2560×1440 viewport — seed 7, detail cap 64):

| | before | after |
|---|---|---|
| planet filling the viewport | 28.2 ms (35 fps) | **17.3 ms (58 fps)** |
| planet at 2× the viewport | 31.6 ms (32 fps) | **9.4 ms (106 fps)** |
| mid-zoom, several worlds past the cap | 39.2 ms (25 fps) | **17.1 ms (58 fps)** |
| compositing one full-screen body | 10.6 ms | **2.2 ms** |
| surface shader, median archetype | 633 ns/px | **361 ns/px** |

**The detail cap is still the biggest single lever**, and it is worth knowing that
it sets a *radius*: cost goes as its square, so halving the slider quarters the
shader work. At the same zoom, cap 32 is 5.8 ms, cap 64 is 17.8 ms and cap 160 is
95 ms. The shipped default of 160 buys full six-octave detail on a screen-filling
world and costs accordingly; drop it if you want the frame back.

## Adding a planet type

Add one row to `TYPES` in `crates/planet-core/src/lib.rs` — palette, thresholds,
flags. The native GIFs and the web demo pick it up automatically; there is only
one copy of the algorithm. To put the new world into orbit as well, add it to
`ROSTER` in `crates/solar/src/lib.rs` (a name + which band it belongs in) —
`solar` renders it with the very same shader.
