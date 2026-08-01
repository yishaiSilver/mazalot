# render-io

The only crate in the workspace that touches `image`: GIF, contact-sheet and poster
helpers for the native generators, plus the parallel GIF encoder that makes them
fast. It owns the workspace's only third-party dependencies — `image`, `gif`,
`rayon` — and is always behind the `native` feature, so a wasm build never sees it.

## encode_gif must stay byte-compatible with `image`

`encode_gif` drives the `gif` crate directly — quantizing frames across cores with
rayon, writing them serially — instead of using `image::codecs::gif::GifEncoder`.
That is what makes the generators ~3.4× faster.

It reproduces `GifEncoder`'s steps **exactly**: speed 1, `delay / 10`, `Background`
disposal, an empty global palette taken from the first frame, `set_repeat` first.
If you touch it, or bump `image` or `gif`, re-run the `out/` hashes — every GIF in
the repo depends on that correspondence, and a drift is invisible by eye.

## Frames must stay independent

The generators run frames through rayon and `collect()` back into order, so output
is deterministic — but only because **every frame closure is a pure function of its
index**. A closure that accumulated across frames would silently produce garbage.

The scene bins are the exception and stay serial on purpose: their
`System`/`Belt`/`Scene` holds `RefCell` caches and is not `Sync`.
