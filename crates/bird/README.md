# bird

A creature generator, fully disjoint from the space crates — it shares nothing with
them but third-party dependencies. Two halves, two bins:

```bash
cargo run --release -p bird --bin alien   # hybrid alien "genus" families
cargo run --release -p bird --bin bird    # naturalistic earth birds

cargo build -p bird --target wasm32-unknown-unknown --release --no-default-features
cp target/wasm32-unknown-unknown/release/bird.wasm crates/bird/web/bird.wasm
cd crates/bird/web && python3 -m http.server 8000
```

- **alien** — seeded hybrids drawn from genus families, so a family reads as
  related creatures rather than as noise. Samples:
  [genus sheet](../../docs/aliens_genus.png),
  [plans](../../docs/aliens_plans.png), [animation](../../docs/aliens_anim.gif).
- **bird** — naturalistic earth birds by archetype. Sample:
  [archetypes](../../docs/birds_archetypes.png).

Both are jointed body plans posed and shaded per frame rather than sphere shaders,
so nothing in [`planet-core`](../planet-core/README.md) or
[`noise-core`](../noise-core/README.md)'s lattice applies. The web demo is the bird
half.
