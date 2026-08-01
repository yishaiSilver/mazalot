# star

One star filling the frame: granulation, sunspots, prominences, corona. The hero
framing of a star, as [`planet`](../planet/README.md) is for a world. The compact
scene tile lives in [`sun-core`](../sun-core/README.md).

```bash
cargo run --release -p star --bin sun    # note: the bin is `sun`, not `star`

cargo build -p star --target wasm32-unknown-unknown --release --no-default-features
cp target/wasm32-unknown-unknown/release/star.wasm crates/star/web/star.wasm
cd crates/star/web && python3 -m http.server 8000
```

## The shading

A star is the **inverse of a planet**: self-luminous, so there is no day/night
terminator and no external light — the whole disc glows. It reuses the shared
`noise-core`/`dither-core` helpers and adds:

- **Granulation** — Worley convection cells (bright centres, dark inter-granular
  lanes) plus a warped-fBm mottle, boiling over time (loop-safe).
- **Sunspots** — low-frequency umbrae drifting slowly across the surface.
- **Limb darkening** — the edge dims and tints cooler (`mu = nz`), which is what
  gives the flat disc its spherical read.
- **Corona** — a soft halo with shimmering radial streamers past the limb.
- **Prominences** — jagged filaments erupting from evenly-spaced limb lobes, each
  firing on its own seamless pulse; flare stars add rare violent spikes.
- **Sparkle motes** — twinkling points in the halo.

## 8 types

Across the temperature spectrum — `blue_giant`, `white_star`, `yellow_dwarf`,
`orange_dwarf`, `red_giant`, `red_dwarf`, `white_dwarf` — plus an exotic teal `sol`
(a nod to *rebels-in-the-sky*).

**Adding a type** is one row in `STYPES`, `src/lib.rs`. A star for a *scene* is a
different table: one row in `SUNS`, [`solar/src/lib.rs`](../solar/README.md).
