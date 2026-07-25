# Standalone artifacts

Every web demo in this repo is normally served as an `index.html` that **fetches
its `.wasm` as a sibling file** (e.g. `crates/solar/web/index.html` loads
`./solar.wasm`). That works great from a local `python3 -m http.server` or from
GitHub Pages, but it needs both files served side by side from the same origin.

An **artifact** is the same demo flattened into **one self-contained HTML file**
with the WebAssembly **inlined as base64**. It runs from anywhere with no server
and no network — double-click it, email it, drop it on any static host, or
publish it as a [Claude artifact](https://claude.ai/code/artifacts). That last
one is the motivating case: sandboxed hosts serve the page under a strict CSP
that blocks cross-file `fetch()`, so the wasm *has* to live inside the HTML.

## Quick start

```bash
scripts/make-artifact.sh solar
# ==> building solar.wasm (release, no-default-features)
# ==> wrote dist/solar.html (≈145 KB, self-contained)
```

Open `dist/solar.html` in a browser and the full interactive demo runs — drag to
pan, scroll/pinch to zoom, all the Controls-dock sliders — with nothing else on
disk.

## Usage

```
scripts/make-artifact.sh <crate> [--out FILE] [--no-build]
```

| Argument      | Meaning                                                             |
| ------------- | ------------------------------------------------------------------- |
| `<crate>`     | a demo under `crates/<crate>/web/` — `solar`, `moon`, `asteroid`, `comet`, `star` |
| `--out FILE`  | output path (default `dist/<crate>.html`)                           |
| `--no-build`  | skip the wasm rebuild; reuse the committed `crates/<crate>/web/<crate>.wasm` |

Examples:

```bash
scripts/make-artifact.sh comet --out /tmp/comet.html
scripts/make-artifact.sh moon --no-build          # use the checked-in wasm as-is
for c in solar moon asteroid comet; do scripts/make-artifact.sh "$c"; done
```

Output lands in `dist/` (git-ignored).

## Requirements

- **Rust** with the wasm target: `rustup target add wasm32-unknown-unknown`
  (only needed when building — `--no-build` skips it).
- **python3** — does the base64 inlining and HTML rewrite.

## How it works

`make-artifact.sh` does three things:

1. **Build** `crates/<crate>/web/<crate>.wasm`
   (`cargo build -p <crate> --target wasm32-unknown-unknown --release --no-default-features`).
   The `--no-default-features` flag drops the native-only `image` dependency so
   the wasm stays tiny. Skipped with `--no-build`.
2. **Strip the document wrapper.** A Claude artifact is published *inside* a
   `<!doctype html><head></head><body>` skeleton, so the script keeps only the
   page content from `<title>` onward and removes the outer
   `<head>`/`<body>`/`</html>` tags.
3. **Inline the wasm.** It base64-encodes the `.wasm`, drops it into a
   `const __WASM_B64 = "…"` at the top of the module script, and rewrites the
   loader from

   ```js
   const res = await fetch("./solar.wasm?v=" + Date.now(), { cache: "no-store" });
   const { instance } = await WebAssembly.instantiate(await res.arrayBuffer(), {});
   ```

   to instantiate straight from the inlined bytes:

   ```js
   const { instance } = await WebAssembly.instantiate(
     Uint8Array.from(atob(__WASM_B64), c => c.charCodeAt(0)), {});
   ```

The result has **no `fetch()` and no external references** — the script asserts
that before writing the file.

## Publishing as a Claude artifact

The generated file is exactly what the Claude Code **Artifact** tool wants
(page content, no outer `<html>`/`<head>`/`<body>`). From a Claude Code session:

> "Publish `dist/solar.html` as an artifact."

You'll get a private `claude.ai/code/artifact/…` URL you can open or share.

## Notes & gotchas

- **Snapshot, not a live view.** The wasm is frozen into the HTML at generation
  time. Change a crate's renderer and you must **regenerate** (just rerun the
  script — it rebuilds the wasm by default).
- **Size.** The HTML grows by ~4/3 of the wasm size (base64). The demos are
  ~45–91 KB of wasm, so artifacts land around 60–150 KB — trivial.
- **Only the `crates/<name>/web/` demos** follow the layout this script expects
  (`solar`, `moon`, `asteroid`, `comet`, `star`). The `planet` and `bird` web
  builds use different package names and directories and aren't wired in here.
- **The landing page (`index.html`) is not artifact-able as-is** — it links to
  each demo's own page and loads several wasm modules for thumbnails, none of
  which survive flattening into one file. Generate the individual demos instead.
