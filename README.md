# mazalot

Procedural, seed-driven pixel-art planets in Rust — **zero art assets**. Every
planet is generated from math per pixel, so a single seed always rebuilds the
exact same world. The core algorithm compiles to both a native GIF/PNG generator
and a ~42 KB WebAssembly module from **one shared codebase**.

There's also a companion **star** generator (a sibling of the planet renderer),
a draggable **solar-system** view that composes a star with orbiting planets, a
paper-doll **character** compositor, and a fully separate **creature** generator
(alien + earth birds) — see below.

## Crate layout

This is a Cargo workspace with **two disjoint halves that share no code** — only
the third-party deps (`image`, `rand`) and this manifest. Planets never touch the
bird crates; birds never touch the planet crates.

**Planets:**

| Crate | What it is |
|-------|------------|
| `core/` (`planet-core`) | The single source of truth: 3D value-noise + Worley, the 26-type planet table, sphere shading, weather, and the pixel-art output stage. **Pure math, zero dependencies.** Emits raw RGBA bytes. Also holds the **star** renderer (`sun` module), which reuses the same noise + dither helpers. |
| `src/` (`sprite-compositor`) | Native generators. Wraps core's frames into spinning **GIFs**, a contact-sheet **PNG**, and a combined all-types GIF (via the `image` crate): `--bin planet`, `--bin sun`. Also the character compositor. |
| `web/` (`planet-web`) | Rust → WASM (raw cdylib, **no wasm-bindgen**). A browser page renders a live rotating planet on a canvas with full tuning controls. |

**Birds (fully disjoint from planets):**

| Crate | What it is |
|-------|------------|
| `bird-core/` (`bird-core`) | Procedural alien/bird creature generation — structural randomness (body plans, features, palettes), not just recolor. **Pure, zero dependencies.** |
| `bird/` (`bird`) | Native generators. `--bin alien` (hybrid alien "genus" families, animated) and `--bin bird` (naturalistic earth birds). |
| `bird-web/` (`bird-web`) | Rust → WASM (raw cdylib, no wasm-bindgen). Renders a live creature on a canvas, with a **Detail** slider that varies the pixelation live (supersamples the same art from chunky to fine). |

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
terminator and no external light — the whole disc glows. The `sun` module reuses
`planet-core`'s noise, color, and Bayer-dither helpers and adds star-specific
shading:

- **Granulation** — Worley convection cells (bright centres, dark inter-granular lanes) plus a warped-fbm mottle, boiling over time (loop-safe).
- **Sunspots** — low-frequency umbrae that drift slowly across the surface.
- **Limb darkening** — the edge dims and tints cooler (`mu = nz`), which is what gives the flat disc its spherical read.
- **Corona** — a soft halo with shimmering radial streamers past the limb.
- **Prominences** — jagged filaments erupting from evenly-spaced limb lobes, each firing on its own seamless pulse; flare stars add rare violent spikes.
- **Sparkle motes** — twinkling points in the halo.

**8 types** across the temperature spectrum — `blue_giant`, `white_star`,
`yellow_dwarf`, `orange_dwarf`, `red_giant`, `red_dwarf`, `white_dwarf` — plus an
exotic teal `sol` (a nod to *rebels-in-the-sky*). Add a star type = add one row
to `STYPES` in `core/src/sun.rs`.

## The solar system

Where `planet` and `star` each render *one* body filling a square, `solar`
(`crates/solar`) renders a **whole system** into an arbitrary rectangular
viewport that you can **drag around** and **zoom into** — a central star with
planets orbiting it, against a starfield. Same seed => the same system, forever.

Like every other "type" crate it is **self-contained** (shares no code — it
carries its own compact noise/color primitives and its own small *tile*
renderers for a star and a planet, scaled to read at the tens-of-pixels size a
system view needs). The new work is the layer on top:

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
- **Space background** — a faint seed-colored **nebula** (baked at low res each
  frame → pixel-art clouds) plus three **parallax** star layers with temperature
  colors. Each layer is a fixed *screen-space* grid scrolled by the camera's
  **accumulated screen-space pan** (Δcam·zoom summed over time) at a fraction `p`
  of the foreground — so on **pan** the stars always move slower than the system
  by the same ratio at every zoom (no runaway when zoomed out), and on **zoom**
  they don't move at all (pure zoom adds no screen displacement, and zoom is
  about the viewport centre). So a star can never move faster than the solar
  system, and the on-screen count stays constant (no wall, no swim). **Star
  density** and **star parallax** controls tune the count and pan scroll rate. Stars are 1px points plotted by iterating the
  visible grid cells — O(cells), not O(pixels). The far layer and the nebula fade
  out (and are skipped) when you zoom in on a body.
- **Click to follow** — click a planet and the camera locks on and tracks it
  around its orbit; drag anywhere to release.

Each frame: paint the background → dot in each orbit path → render every body to
a small RGBA tile and alpha-blend it in, depth-sorted. Bodies are small, so the
whole scene stays cheap enough to render live *while you drag*.

**Add a planet archetype** = add a row to `PKINDS`; **add a star** = add a row to
`SUNS` — both in `crates/solar/src/lib.rs`.

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
- **Layout** — planet count, planet spacing, planet size, sun size.
- **Motion** — orbit speed, and separate **planet** and **star rotation** speeds
  (three independent clocks; each accumulates so changing a speed never jumps).
- **Pixelation** — scene / planet / sun pixel size, plus per-body **detail caps**
  (planets and sun separately): the "lower bound of pixelation" — how far you can
  zoom before a body stops resolving finer and just enlarges its blocks. Lower
  caps also keep zoomed-in views cheap.
- **Background** — **star density** (how many background stars, constant across
  zoom) and **star parallax** (scroll-rate multiplier: 0 pins the stars on pan,
  higher makes them scroll faster / feel closer).

Sizes/spacing/pixelation/detail-caps are live view params applied to the system
(`system_set_view`) with no regeneration; only seed and planet count rebuild it.
Off-screen bodies are culled and each body's tile is bounded, so zoom stays
responsive. Works on touch/mobile. (`node verify.mjs` renders the system
headlessly as a build check.)

**Web — the solar-system companion demos (moons, asteroid belt, comet, orbits):**
```bash
for c in moon asteroid comet orbit; do
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
- **orbit** — genuine eccentric, inclined Keplerian orbits (star at a focus); a
  **Follow body** button tracks a world and shows its eccentricity in the HUD.

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

## Adding a planet type

Add one row to `TYPES` in `core/src/lib.rs` — palette, thresholds, flags. Both
the native GIFs and the web demo pick it up automatically; there is only one copy
of the algorithm.
