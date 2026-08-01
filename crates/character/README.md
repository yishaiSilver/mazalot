# character

A paper-doll character compositor: seeded bodies, heads and gear layered into one
sprite. **Native only** — no wasm face, no web demo, no `lib.rs`.

```bash
cargo run --release -p character --bin character
```

Sheets land in `out/`; [`docs/characters.png`](../../docs/characters.png) is a
sample.

Unlike the space crates, nothing here is a sphere shader — parts are generated and
stacked, so the interesting constraints are ordering and anchor points rather than
noise budgets.
