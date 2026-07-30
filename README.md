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
  (The web demos freeze the billowing by default — see *Frozen weather*.)
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

Caching does mean the cost arrives as a **spike**: most frames are a blit and one
in ~5 pays the whole re-bake, so what matters for the star is the size of that
one frame, not the average. Two things shrank it, and neither is the planets' fix
— the star's fBm fields are so low-frequency that their octave counts are
*already* under Nyquist at any scene radius:

- **Its corona is the majority of the tile.** The halo annulus out to
  `1 + corona_reach` radii is ~1.9× the disc's area, so ~65% of the shaded pixels
  are corona (71.9k vs 38.0k at a 110 px radius) — and every one of them was
  running a two-octave fBM and a `powf` to evaluate something that varies along
  *one* axis. The streamers depend only on the angle around the limb, the falloff
  only on the distance past it, and the disc's limb darkening only on `mu`, so
  `sun_core::Shade` samples each along its own axis once per bake and interpolates.
  Indexing the angular one needs a monotone angle, which `diamond_angle` gives for
  a divide and a compare instead of an `atan2`.
- **The old code recovered an angle with `atan2` only to feed it straight back
  through `cos`/`sin`.** Out past the limb the unit direction is just `(nx, ny)/r`
  — same field, three transcendentals cheaper, on those same 65% of pixels.

Sun alone, in the browser at 1680×944, cap 110: **22.7 → 15.8 ms** per frame at a
screen-filling zoom, **20.7 → 7.6 ms** when it overflows the viewport, with the
re-bake spike roughly 59 → 38 ms. `sun-core`'s tests pin the tabulated fields
against direct evaluation — exact to the byte from a 24 px radius up.

Everything invalidates automatically the moment its key changes.

**Zoomed-in planets.** Planets can't be tile-cached the way the star is — their
axial rotation *is* the visible motion, and at any usable quantum the tile
changes every frame anyway. So the zoomed-in case is attacked by doing less work
per frame rather than by reusing frames, in three independent ways:

- **Shade only what the compositor will read.** `scene_core::visible_tile_rect`
  asks where a tile actually lands on screen and returns the sub-rect `blit` will
  sample; `planet_core::render_tile_into` shades that and nothing else. A disc
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

### Is the browser using your cores?

A wasm *instance* is one thread, so a single instance uses exactly one core
however many the machine has — and that, not the instruction set, is the largest
gap left to native. On the measurements here wasm runs within roughly 1.3x of
native single-threaded; the generators then fan frames across every core with
rayon. The `planet` and `solar` demos now fan a frame across a **worker pool**
instead, which closes most of it; see the numbers below and the caveat in
[WebGL2](#webgl2-the-same-shader-on-the-gpu) about what the pool does *not*
reach.

Two things get called multithreading on the web, and only one of them is gated:

| | needs COOP/COEP headers | works on GitHub Pages |
|---|---|---|
| shared-memory threads (`SharedArrayBuffer`, what a rayon port compiles to) | yes | **no** — Pages cannot set response headers |
| worker-per-region + transferable `ArrayBuffer` | no | **yes** |

The shader is a pure function of pixel position, so it wants the second one:
each worker renders a band into its own buffer and transfers it back with no
copy. `scripts/make-parallel-probe.sh` builds a single self-contained page that
checks the parts worth checking rather than assuming them — whether a strict CSP
forbids blob-URL workers (the only way a one-file build gets off the main
thread), whether `simd128` is there, and what the cores are really worth. It
runs the actual `planet.wasm`, not a synthetic loop.

Measured in this container (4 shared cores), a 256px planet with every switch on:

| | ms/frame | fps |
|---|---:|---:|
| 1 worker — what the demos do today | 17.79 | 56 |
| 4 workers | 6.10 | 164 |

**2.9x, at 73% of four cores**, with 0.18 ms of dispatch overhead against a 6 ms
frame — so band-splitting costs ~3% to set up, not enough to eat the win. The
probe prints throughput and dispatch separately on purpose: throughput is the
optimistic number, and the dispatch cost is the part it hides.

### Feature cost lab

The `planet` demo carries an ablation panel: a tick-box per shader feature, and a
button that switches each one off in turn and times the difference. It measures
on *your* machine, so the numbers below are a reference point rather than a
claim about your hardware.

Most features are reachable through the existing per-type sliders — `clouds`,
`specular`, `spot`, `aurora`, `lightning`, `storm_cells` and `caps` are all gated
on `> 0.0`, so zeroing one switches it off. The `F_*` mask in `planet-core`
covers what a parameter cannot reach: part of a layer rather than a whole one
(the cloud self-shadow), framing furniture (atmosphere rim, dark limb, starfield)
and the two optimizations. `render_rgba_features(.., F_ALL, ..)` is byte-identical
to `render_rgba_styled`, so nothing about the normal path changes.

Measured here, `terran` at 64² (full frame 1.449 ms):

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

The cloud deck is the whole game on a terrestrial world — everything else put
together is about a quarter of the frame. Two things stand out as poor value:
**aurora costs 12% for a thin polar band**, and **specular is 19% of a `lava`
frame** for a glint whose intensity is 0.05. By archetype: `gas_giant` is great
spot 16%, `lava` is specular 19% and nothing else, `barren` has no feature above
5% because it is all Worley, which has no switch.

For comparison the same panel reports what the optimizations are worth in this
framing: cheap warp saves 19% on `terran`, 31% on `gas_giant`, 51% on
`storm_shroud`; night-side thinning saves 4% on `terran`, 11% on `ocean`.

### Frozen weather

The cost table above says a cloudy world *is* its cloud deck. Per pixel the deck
is 14 `value_noise` evaluations — a 4-octave domain warp for the tops (3 inner
displacement fields + 1 outer) plus a plain 4-octave field for the self-shadow.
All 14 collapse into two table reads the moment the deck stops **evolving**.

The deck already turns at 2× the surface, which is the parallax that makes
weather read as its own layer. What costs per-frame work is not the rotation but
the billowing (a periodic morph) and the churning storm cells — both driven by
`angle`. Freeze those and the density becomes a fixed function of a direction on
the sphere, so it can be baked once into an equirectangular map in
(longitude, y) and sampled for every frame after.

`y` rather than latitude is deliberate: a sphere point is
`(r·cos θ, y, r·sin θ)` with `r = √(1−y²)`, so a map row is exactly a circle of
constant `y` and the vertical axis costs no transcendental at lookup time. It is
also equal-area, so texels carry uniform detail instead of piling up at the
poles. The map is `u8` — it feeds an 0.18-wide `smoothstep`, so one quantum
moves the result by 2.2% of that ramp against a dither step of 0.045. Width is
`4·rad` rounded up to a power of two and capped at 1024, which puts about one
texel on one pixel and lets the adaptive detail cap nudge the radius without
re-baking.

Two kinds of planet get one, and the second is the bigger win:

| | what is frozen | native | wasm |
|---|---|---:|---:|
| **deck** over a solid surface (`clouds > 0`) | the tops + the shadow field | 1.85–1.96× | 1.6–1.7× |
| **shroud** that *is* the surface (`Base::Cloudy`) | the whole band/turbulence mix factor, so the lookup lands one `mix` from the pixel | 2.6× | 1.76–1.85× |

The shrouded worlds do better because their entire surface algorithm collapses
into the plane — the baked value is the finished mix factor, not an input to
more math. wasm gains less than native across the board: its noise kernels are
four-lane SIMD while the table read is scalar byte loads, so the thing being
replaced was relatively cheaper there to begin with.

In the scene, following a cloudy planet that fills a 1000×640 view:
**59.3 → 29.9 ms, 17 → 33 fps (1.98×)**.

The bake costs about five frames and then pays for the rest of the animation —
that ratio holds at every size, because the map scales with the render. It is
kept in a small per-thread LRU (8 slots / 12 MB); a scene draws every planet on
the way to drawing one, so a one-deep cache would evict on every body and
re-bake on the next, turning the optimization into a pessimization.

What it costs to look at: on `terran`, 17.7% of pixels move by a mean of
6.8/255. That number is not blur — the map is built at the same octave count the
live path would have used, so the deck is exactly as detailed. It is a
*different* sky: frozen at one phase, with storm cells wound to a fixed state
(`STORM_STATIC`) rather than churning. Worlds with thinner decks move less
(`tundra` 8.9%, `desert` 5.6%).

#### The same trick on the surface

Once the map exists, the question is what else is a pure function of a direction
on the sphere. The answer is most of the shader: `Base::Terrestrial` and
`Base::Cratered` contain no `angle` term at all — 15 of the 26 types. So
`F_BAKED_SURFACE` bakes their albedo into the same map, RGB interleaved.

The two that stay live are the ones that genuinely advect: a gas giant's zonal
jets drift by `angle · 0.16 · sin(lat · bands / 2)` and a lava world's glow is
carried by a flow field. Freezing those would stop the bands sliding past each
other, which is the whole look, so they keep paying.

wasm, 160px tile, cumulative:

| type | live | + frozen deck | + baked surface | total |
|---|---:|---:|---:|---:|
| `terran` | 91 fps | 155 fps | **215 fps** | 2.36× |
| `ocean` | 95 fps | 167 fps | **222 fps** | 2.33× |
| `barren` | 206 fps | 205 fps | **472 fps** | 2.29× |
| `moon` | 206 fps | 207 fps | **464 fps** | 2.25× |
| `iron` | 178 fps | 178 fps | **247 fps** | 1.39× |
| `gas_giant` / `lava` | — | — | — | 1.00× |

The cratered worlds double because Worley searches 27 cells per pixel and there
is nothing else in their frame; `iron` gains least of the terrestrials because
it has no cloud deck to have frozen first. In the scene, following a cloudy
planet that fills a 1000×640 view: **52.2 → 18.5 ms, 19 → 54 fps (2.82×)**.

It costs far less to look at than the frozen deck does — 1.3–5.6% of pixels move
by a mean of 0.15–1.11/255, against the deck's 17.7% and 6.8. Nothing is
*frozen* here that was moving; the only error is the `u8` texel and the bilinear
filter. That filter is the one thing to watch: `ramp` is a hard step function, so
every coastline is a colour discontinuity, and blending across one produces a
shade that is not in the palette. At the width this bakes — about one texel per
pixel — the filter is near identity and the difference stays on coastline pixels.
Past the 1024-texel cap it will start to soften them, exactly where you are most
zoomed in.

#### The two families that advect

That leaves the gas giants and the lava worlds, whose fields genuinely move.
Neither needed to be frozen — they needed to be *decomposed*, and the split is
different in each.

`Base::Emissive` separates cleanly. Its 6-octave rock field `n` is static and
bakes; the 3-octave `flow` that lights it advects in three dimensions — the
field *evolving*, not moving — so it stays live at full rate. Six of nine
octaves go and the glow still flows: **1.39–1.50×**, with nothing lost.

`Base::Banded` is a coordinate change rather than an overlay. Its drift,
`angle · 0.16 · sin(lat · bands / 2)`, is added to the sample's **x**, which
slides the field through the sphere and so cannot be a lookup offset. Re-express
the same rate as a rotation in *longitude* and the sample stays on the sphere:
animating the bands becomes one subtraction from the texture coordinate. The
bake is exact under that model, because `band` is a function of the warp and of
`y`, and a longitude shift leaves `y` alone. Two planes, since the band and
fine-detail fields drift at different rates (1.0 and 1.4). **1.86–2.03×**.

`F_BAKED_BANDS` is its own switch because it changes what the motion *is*: the
bands counter-rotate instead of shearing past each other. Measured, the two
models differ by about as much as one frame of the animation differs from the
next — 8.9% of pixels at mean 1.07/255 between models, against 9.1% between
consecutive frames of the old one — and the new model animates at the same rate
(9.0% frame to frame). So it reads as the same planet a moment later, not as a
different planet. Whether it is *better* is a look question; the code comment
has always described the intent as "adjacent latitude bands drift in opposite
directions", which is what the rotation model actually does.

Every type in the table is now covered by one of the three switches, and each
works on its own:

| switch | types |
|---|---|
| `F_BAKED_CLOUDS` | 12 — every world with a deck, plus the two shrouded ones |
| `F_BAKED_SURFACE` | 20 — Terrestrial, Cratered, Emissive |
| `F_BAKED_BANDS` | 4 — the gas giants |

#### Putting the billowing back

Freezing the deck cost two things: the storm swirl and the billowing morph. The
morph is the one that mattered — it is what made weather form and dissipate
rather than slide — and it is also the one nothing so far could express, because
it translates the noise domain in y *and* z. That is the field evolving, not
moving, and no lookup offset represents it.

But a *stack* of maps does. `F_MORPH_LUT` bakes the deck at six points across
the morph cycle and interpolates between the two that bracket the current value.
The table is indexed by the morph value rather than by time, since it oscillates
rather than advancing, so the lookup walks back and forth across six planes
instead of running off the end of an ever-growing one.

Adjacent phases are well correlated at the coarse octaves and independent at the
fine ones, which is exactly what makes the interpolation read as a dissolve —
cloud forming and dissipating — instead of a slide. Measured against the live
deck, over a full spin:

| | mean delta from live |
|---|---:|
| frozen | 7.56/255 |
| **+ morph LUT** | **2.98/255** |

**It recovers 61% of what freezing gave up, for 4% of the frame** (`terran`
2.24× → 2.16×). The cost is memory and bake time, both 6×: at full width the
deck is 6 MB for one planet, and the first frame at a new zoom pays six bakes
instead of one.

The storm swirl stays frozen. It runs on its own cycle, independent of the
morph, so restoring it too would need the product of the two axes rather than
the sum — 36 planes, not 12.

Because they change the picture rather than the pixel budget, none of these
switches is in `F_ALL` — which is `F_CLOUD_SHADOW | F_ATMO | F_RIM |
F_STARFIELD | F_CHEAP_WARP` and nothing else. That is what keeps `out/`
byte-identical while the web demos run with the lot.

Measured against the shipped renderer (which already sizes octaves to the pixel
grid and clips tiles to the viewport), a 160px tile in wasm:

| type | shipped | + every switch | |
|---|---:|---:|---:|
| `barren` | 200 fps | **459 fps** | 2.30× |
| `ocean` | 103 fps | **231 fps** | 2.26× |
| `gas_giant` | 112 fps | **220 fps** | 1.97× |
| `terran` | 101 fps | **210 fps** | 2.08× |
| `toxic` | 154 fps | **310 fps** | 2.01× |
| `lava` | 214 fps | **282 fps** | 1.32× | The native generators keep the live shader and `out/` is byte-identical; the
three web demos turn all three on at construction, behind one `Frozen weather`
checkbox each, alongside night-side thinning. The planet lab exposes them
separately (`Night-side thinning`, `Frozen cloud deck`, `Baked surface`,
`Baked bands`, `billow (morph LUT)`) so each can be A/B'd on its own — the last
is nested under the deck, which it needs.


### WebGL2: the same shader on the GPU

Everything above divides the frame. The worker pool splits a scene's *bodies*
across cores, but the backdrop is full-screen serial work that scales with the
window and the pool never touches it — and a camera following a planet
invalidates the backdrop cache every frame, so it really is repainted. That is
an Amdahl term, and no number of workers removes it. Moving the whole frame to
the GPU does.

The port is far cheaper than it sounds, for a reason worth stating plainly:

> **`hash3` and `value_noise` are `u32` integer math.** Wrapping multiplies,
> xors, shifts — no transcendentals, nothing a driver is free to round its own
> way. They transliterate into GLSL ES 3.00 *exactly*, so the lattice under the
> GPU picture is bit-identical to the lattice under the CPU one. Worley and the
> fBm stack fall out of that for free.

So there is no "port `noise-core` to the GPU" problem. What is actually rewritten
is ~200 lines of ramps, mixes and smoothsteps, in
`crates/planet-core/src/shader.glsl`. Everything else stays in Rust:
`gl_uniforms()` computes the `PType` row with the slider overrides applied, the
seed offsets, the vortex centres, this frame's moons, the key light, the colour
ramp, the palette and the whole `Lod` octave budget, and ships them as one flat
float array. **The type table is transported, not duplicated** — adding a planet
type is still one row, and the GPU picks it up.

The shader source lives inside `planet.wasm` (`gl_shader_ptr`/`gl_shader_len`),
so it cannot go stale against the module it was built with and the single-file
artifact keeps working.

It runs the **live** shader — no `F_BAKED_*`. Those bakes exist to make a CPU
afford the weather; they cost frozen storms, and in a worker pool they cost a
private ~7.5 MB copy of the sphere maps *per instance* (60 MB at 8 workers),
because workers cannot share memory without COOP/COEP. A GPU would rather
evaluate the noise. So the GPU path is both faster and less frozen.

**Verification.** `out/` is the regression test for the native path;
`scripts/verify-gl.mjs` is its equivalent here. It renders both paths in headless
Chromium and diffs them per pixel. All 26 types, 4 angles × 2 seeds, 128px:

| | |
|---|---|
| types bit-identical to the wasm renderer | **15 of 26** |
| worst per-type pixel disagreement | **0.09%** (`ocean`) |
| pixels differing by more than one quantization level | **0.00%** |

`--demo solar` diffs whole scenes the same way, at three zooms (fit, mid, and
zoomed onto a body), and also lands at **0.00%** past a level. Its raw
disagreement rate is much higher — 17–30% of pixels — and that is the nebula
rather than the bodies: at a zoom where the clouds have faded out, the backdrop
is **byte-exact**. The GPU evaluates the cloud fBm per *pixel* (quantized to the
same 8x8 lattice) where the CPU bakes it per *cell*, so the two round differently
right at the density threshold. Nothing is wrong with the picture; the numbers
just are not the same numbers. See below for the version of that which would also
be faster.

Every differing pixel differs by exactly one 22-level step (12/255). That is the
signature of a value landing on the other side of a quantizer threshold, not of
a shading bug: ANGLE is free to round `sin`, `exp`, `pow` and `sqrt` its own way,
and a 1e-7 difference before `quant` becomes a whole level after it. The types
that come out *exactly* equal are the ones whose shading is ramps and steps with
no transcendental in the path — `barren`, `moon`, `lava`, `desert`, `chrome`.

**The whole scene, not just a disc.** `solar` renders on the GPU in three passes
— one fullscreen triangle for the backdrop, the dashed orbit paths as point
sprites, then one quad per body back-to-front with alpha blending, which *is*
the painter's algorithm `blit` was implementing by hand. Rust still owns the
scene: `gl_bodies()` hands over a draw list whose every number comes from the
same expressions `draw_bodies_band` uses (`Planet::at`, `to_screen`,
`dest_rect`, the detail caps, the sunward light), and each record carries that
shader's uniform block.

Everything the CPU path had to cache is simply absent:

| CPU | GPU |
|---|---|
| `BackdropCache` — a scrolling ground/nebula sprite, memmoved on a pan | one triangle; a camera that follows a body costs the same as a still one |
| `SunCache` + a quantized boil clock, so a costly tile bake can be reused | no bake, so `t_sun` passes straight through and the convection stops stepping |
| `visible_tile_rect`, hand-arranging which tile pixels get shaded | the rasterizer clips the quad |
| ~32 MB/frame of copies: band slices, worker transfers, `putImageData` | nothing is read back |

The per-body pixelation knobs still work, and exactly: the fragment shader maps
its destination pixel back through the *same* expression `blit` uses, so a body
is blocky in the places `planet_pixel` and the detail cap make it blocky, with no
second render target.

**The HUD says what it is running on.** Under WebGL2 the perf readout adds the
adapter name, the draw counts, and — where the browser allows it — the real GPU
time per frame:

```
36 fps · 1.1 ms render
submit 1.1 ms (GPU runs async)
backdrop: no cache needed
GPU Apple M2 · gpu 2.31 ms · 14% of a 60 fps slot
draw 5 bodies · 2410 stars · 436 orbit
```

Three things there are deliberate. `render` is relabelled **submit** because the
draw calls return long before the GPU has finished them, so timing them and
calling it CPU load would flatter the path. The GPU time comes from
`EXT_disjoint_timer_query_webgl2`, read a few frames late so asking never stalls
the pipeline — Chrome exposes it, Firefox removed it and Safari never shipped it,
so its absence is normal and says so. And a **software rasterizer is called out
in amber**: `gl.RENDERER` is masked to something generic by every modern browser,
so the real name needs `WEBGL_debug_renderer_info`, and a browser that has
quietly fallen back to SwiftShader or llvmpipe is running this path with none of
its advantages — which is the single most useful thing the HUD can tell you when
the GPU renderer is somehow *slower* than the CPU one.

The timer is sanity-checked against the frame interval before it is believed: a
frame cannot spend more GPU time than the wall clock between frames, so a reading
far past that is the driver misbehaving. SwiftShader here reports ~750 ms against
50 ms frames, which is how the check came to exist. It matters beyond the
display, because auto-detail paces off that number and an inflated one would peg
the detail cap to the floor.

There is no WebGL API for GPU *utilization* or VRAM — browsers do not expose
either, and none of the fingerprinting-adjacent tricks are worth it. GPU
milliseconds against the vsync slot is the honest version of the same question.

**What this repo cannot tell you: whether it is faster.** This container has no
`/dev/dri`, so the only WebGL2 available is ANGLE over SwiftShader — a *software*
rasterizer. It proves the shader is right and says nothing about GPU throughput;
timing it would be timing the CPU. The demo's `Renderer` dropdown is there so you
can run the A/B on hardware that has a GPU.

```bash
node scripts/verify-gl.mjs                       # planet, 9 types, 96px
node scripts/verify-gl.mjs --demo all --types all
node scripts/verify-gl.mjs --demo solar --size 240
```

### Scatter, don't gather

The first version of the backdrop shader drew the stars in the fragment shader:
each pixel asked which of the nine surrounding cells, in each of three parallax
layers, might have placed a star on it. That is the only way a *fragment* can ask
the question, and it is 27 hashes per pixel across the whole screen — against
`paint_stars`, which walks the lit cells and plots one pixel each, roughly one
hash per fifty pixels. **A thousand times the work for the same picture.**

It was three quarters of the fragment cost. Under SwiftShader (a software
rasterizer, so these are CPU numbers — but the ratio is fragment ALU either way),
800x500, fit view:

| | ms/frame | fps |
|---|---:|---:|
| stars gathered in the fragment shader | 216.6 | 4.6 |
| **stars scattered as point sprites** | **50.0** | **20.0** |

With the stars as points, switching the starfield off entirely now changes the
frame by *nothing measurable* — which is the check that says the cost really
moved rather than merely shrank.

The fix reuses what the orbit paths already did: `visit_stars` is the one cell
walk, `paint_stars` plots pixels from it, `gl_star_points` emits vertices from
it, both into the same `(x, y, r, g, b)` buffer under an additive blend. The
lesson generalizes past this repo — **when porting a scatter to a shader, look
for the vertex path before you write the gather.**

The nebula is the same shape of problem, still unfixed: `BackdropCache` bakes one
fBm sample per 8x8 cell and scrolls the sprite, where the fragment shader
recomputes that per-cell value at every pixel — 64x the noise evaluations. Far
milder than the stars' 1000x, and not the bottleneck now. If it becomes one, the
fix is again to port the sprite rather than thin the shader: bake the cell field
into a low-res texture, scroll it, repaint only the exposed strip, and let
hardware sampling do the rest. Same idea, one level down.

## Adding a planet type

Add one row to `TYPES` in `crates/planet-core/src/lib.rs` — palette, thresholds,
flags. The native GIFs and the web demo pick it up automatically; there is only
one copy of the algorithm. To put the new world into orbit as well, add it to
`ROSTER` in `crates/solar/src/lib.rs` (a name + which band it belongs in) —
`solar` renders it with the very same shader.
