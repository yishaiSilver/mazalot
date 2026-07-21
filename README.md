# mazalot

Procedural, seed-driven art generation in Rust — with **zero art assets**.
Everything (planets, characters) is generated from math, so a single seed always
rebuilds the exact same result. The core algorithm compiles to both a native
generator and a ~32 KB WebAssembly module from **one shared codebase**.

## What's inside

| Crate | What it is |
|-------|------------|
| `core/` (`planet-core`) | The single source of truth: 3D value-noise + Worley, the 26-type planet table, sphere shading, rings, specular glare. Pure math, zero dependencies. Outputs raw RGBA bytes. |
| `src/` (`sprite-compositor`) | Native generator. Turns core's frames into spinning **GIFs**, a contact-sheet **PNG**, and a combined all-types GIF (via the `image` crate). Also a paper-doll **character** compositor. |
| `web/` (`planet-web`) | Rust → WASM (raw cdylib, no wasm-bindgen). A browser page renders a **live rotating random planet** on a canvas with tuning sliders. |

### Planets
26 types across 5 base algorithms (terrestrial, cratered gas/ice giants,
emissive lava/fungal, cloud-shrouded), plus **rings** and per-material
**specular glare**. The "3D" is per-pixel: treat the disc as a sphere's front
hemisphere, rotate the surface point around Y, sample 3D noise there, shade
against a fixed light. A full 360° spin loops seamlessly.

### Characters
A paper-doll compositor: tiny hand-authored parts drawn in R/G/B placeholder
markers, recolored per-layer via seeded color maps and composited — a small
part library × recolor × layers = effectively unlimited unique characters.

## Running it

**Native — generate GIFs + PNG into `out/`:**
```bash
cargo run --release --bin planet            # planets
cargo run --release --bin sprite-compositor # characters
```

**Web — live rotating random planet:**
```bash
cargo build -p planet-web --target wasm32-unknown-unknown --release
cp target/wasm32-unknown-unknown/release/planet_web.wasm web/planet.wasm
cd web && python3 -m http.server 8000       # open http://localhost:8000/
```
(Requires the wasm target: `rustup target add wasm32-unknown-unknown`.)

## Adding a planet type

Add one row to `TYPES` in `core/src/lib.rs` — palette, thresholds, flags. Both
the native GIFs and the web demo pick it up automatically; there's only one copy
of the algorithm.
