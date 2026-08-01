# moon

A [`planet-core`](../planet-core/README.md) world with moons in orbit around it,
depth-sorted so they pass in front of and behind the parent.

```bash
cargo run --release -p moon --bin moon

cargo build -p moon --target wasm32-unknown-unknown --release --no-default-features
cp target/wasm32-unknown-unknown/release/moon.wasm crates/moon/web/moon.wasm
cd crates/moon/web && python3 -m http.server 8000
```

A sibling of [`solar`](../solar/README.md): the same draggable camera, the same
`background-core` sky, the same constant-block pixel-art scheme. Sliders for moon
count, orbit speed and scene pixelation.

Both the parent and its moons are the one planet shader in its *sprite* framing —
each moon is lit from the parent's screen direction and blitted depth-sorted, which
is the same machinery `solar` uses for worlds around a star.

`PARENTS` (`src/lib.rs`) names `planet-core` types **by string**, and a typo
silently falls back to type 0 — this crate's tests exist to catch that. The
`PARENT_NAMES` array in `web/index.html` mirrors it by hand and nothing checks the
lengths agree.

`MoonSystem::night_lod` is off by default: it changes the image, so it lives outside
`F_ALL` and the generator does not get it.
