# asteroid

A drifting, perspective-squashed asteroid belt.

```bash
cargo run --release -p asteroid --bin asteroid

cargo build -p asteroid --target wasm32-unknown-unknown --release --no-default-features
cp target/wasm32-unknown-unknown/release/asteroid.wasm crates/asteroid/web/asteroid.wasm
cd crates/asteroid/web && python3 -m http.server 8000
```

A sibling of [`solar`](../solar/README.md) — same draggable camera, same
[`background-core`](../background-core/README.md) sky, same constant-block
pixel-art scheme. Live `belt_set_view` sliders: rock count, spacing, rock size,
star density, and a centre-marker toggle.

Rocks are not tiles. A distant one is a single depth-shaded speck (`plot_speck`)
and a near one is a small procedural sprite (`plot_sprite`), both plotted straight
into the frame — so a belt's cost tracks how many rocks land on screen, not how
wide it is. `belt_density` carves the gaps, and depth shading is what sells the
perspective squash.
