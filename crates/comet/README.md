# comet

A comet on an eccentric orbit with an anti-sunward tail, whipping through
perihelion past a [`sun-core`](../sun-core/README.md) star.

```bash
cargo run --release -p comet --bin comet

cargo build -p comet --target wasm32-unknown-unknown --release --no-default-features
cp target/wasm32-unknown-unknown/release/comet.wasm crates/comet/web/comet.wasm
cd crates/comet/web && python3 -m http.server 8000
```

A sibling of [`solar`](../solar/README.md) — the same draggable camera, the same
[`background-core`](../background-core/README.md) sky, the same constant-block
pixel-art scheme. **Follow comet** locks the camera to the head, which is the case
worth watching: the tail always points away from the star, so it swings through a
half-turn as the comet rounds perihelion and is longest and brightest there.

No `planet-core` dependency — the head and tail are their own shading over the
shared noise, with the star tile composited behind.
