#!/usr/bin/env bash
#
# make-artifact.sh — bundle a crate's web demo into a SINGLE self-contained HTML
# file with its WebAssembly inlined as base64, so it runs from anywhere with no
# network access (open it locally, host it statically, or publish it as a Claude
# artifact). See docs/artifacts.md for the full story.
#
# Usage:
#   scripts/make-artifact.sh <crate> [--out FILE] [--no-build]
#
#   <crate>       a demo under crates/<crate>/web/ (solar, moon, asteroid, comet, star)
#   --out FILE    output path (default: dist/<crate>.html)
#   --no-build    skip the wasm rebuild; use the committed crates/<crate>/web/<crate>.wasm
#
# Examples:
#   scripts/make-artifact.sh solar
#   scripts/make-artifact.sh comet --out /tmp/comet.html
#   scripts/make-artifact.sh moon --no-build
#
set -euo pipefail

die() { printf 'make-artifact: %s\n' "$1" >&2; exit 1; }

# --- repo root (this script lives in scripts/) --------------------------------
ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

# --- parse args ---------------------------------------------------------------
CRATE=""
OUT=""
BUILD=1
while [ $# -gt 0 ]; do
  case "$1" in
    --out)      OUT="${2:-}"; [ -n "$OUT" ] || die "--out needs a path"; shift 2 ;;
    --no-build) BUILD=0; shift ;;
    -h|--help)  sed -n '2,20p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    -*)         die "unknown option: $1" ;;
    *)          [ -z "$CRATE" ] || die "unexpected extra argument: $1"; CRATE="$1"; shift ;;
  esac
done
[ -n "$CRATE" ] || die "usage: scripts/make-artifact.sh <crate> [--out FILE] [--no-build]"

HTML="$ROOT/crates/$CRATE/web/index.html"
WEBDIR="$ROOT/crates/$CRATE/web"
PRIMARY="$WEBDIR/$CRATE.wasm"
[ -f "$HTML" ] || die "no demo at crates/$CRATE/web/index.html (try: solar, moon, asteroid, comet, star, galaxy)"

OUT="${OUT:-$ROOT/dist/$CRATE.html}"

command -v python3 >/dev/null 2>&1 || die "python3 is required (used to inline the wasm)"

# Some demos load MORE than their own wasm (e.g. galaxy embeds solar for the
# drill-down, via a `loadWasm(url)` helper). Treat every *.wasm sibling in the
# web dir as a module to inline; the crate to (re)build is the file's basename.
shopt -s nullglob
WASMS=()
for w in "$WEBDIR"/*.wasm; do WASMS+=("$w"); done
shopt -u nullglob

# --- build the wasm(s) (unless told to reuse the committed ones) --------------
if [ "$BUILD" -eq 1 ]; then
  command -v cargo >/dev/null 2>&1 || die "cargo not found; install Rust or pass --no-build"
  if ! (rustup target list --installed 2>/dev/null | grep -q wasm32-unknown-unknown); then
    die "wasm target missing — run: rustup target add wasm32-unknown-unknown (or pass --no-build)"
  fi
  # Always build the primary crate; also rebuild any sibling-module crates
  # (their .wasm already sits in the web dir — e.g. solar for galaxy).
  BUILD_LIST=("$CRATE")
  for w in "${WASMS[@]}"; do
    name="$(basename "$w" .wasm)"
    [ "$name" = "$CRATE" ] && continue
    BUILD_LIST+=("$name")
  done
  for name in "${BUILD_LIST[@]}"; do
    echo "==> building $name.wasm (release, no-default-features)"
    ( cd "$ROOT" && cargo build -q -p "$name" --target wasm32-unknown-unknown --release --no-default-features )
    cp "$ROOT/target/wasm32-unknown-unknown/release/$name.wasm" "$WEBDIR/$name.wasm"
  done
  shopt -s nullglob; WASMS=(); for w in "$WEBDIR"/*.wasm; do WASMS+=("$w"); done; shopt -u nullglob
fi
[ -f "$PRIMARY" ] || die "no wasm at crates/$CRATE/web/$CRATE.wasm (drop --no-build to build it)"

# Pass the primary wasm first (the single-module path uses it), then the rest.
ORDERED=("$PRIMARY")
for w in "${WASMS[@]}"; do [ "$w" = "$PRIMARY" ] || ORDERED+=("$w"); done

# --- inline the wasm(s) + strip the outer document wrapper --------------------
mkdir -p "$(dirname "$OUT")"
python3 - "$HTML" "$OUT" "${ORDERED[@]}" <<'PY'
import base64, os, re, sys

html_path, out_path, wasm_paths = sys.argv[1], sys.argv[2], sys.argv[3:]
html = open(html_path, encoding="utf-8").read()
def b64(p): return base64.b64encode(open(p, "rb").read()).decode("ascii")

# The Artifact host supplies <!doctype html><head></head><body>; keep only the
# page content from <title> onward, and drop the closing wrapper tags.
i = html.find("<title>")
if i == -1:
    sys.exit("make-artifact: no <title> found in %s" % html_path)
html = html[i:]
for tag in ("</head>", "<body>", "</body>", "</html>"):
    html = html.replace(tag, "")

if '<script type="module">' not in html:
    sys.exit('make-artifact: expected a <script type="module"> block')

if "async function loadWasm" in html:
    # Multi-module demo: a `loadWasm(url)` helper fetches each sibling wasm.
    # Inline them all into a url->base64 map and instantiate from the map.
    entries = ",\n".join('  "./%s": "%s"' % (os.path.basename(p), b64(p)) for p in wasm_paths)
    html = html.replace('<script type="module">',
                        '<script type="module">\nconst __WASM = {\n%s\n};' % entries, 1)
    html = re.sub(r'\n[ \t]*const res = await fetch\([^;]*\);', "", html, count=1)
    html = re.sub(r'\n[ \t]*if \(!res\.ok\)[^;]*;', "", html, count=1)
    html = html.replace("await res.arrayBuffer()",
                        "Uint8Array.from(atob(__WASM[url]), c => c.charCodeAt(0))")
    token = "__WASM"
else:
    # Single-module demo: one top-level fetch of the crate's own wasm.
    html = html.replace('<script type="module">',
                        '<script type="module">\nconst __WASM_B64 = "%s";' % b64(wasm_paths[0]), 1)
    html = re.sub(r'\n[ \t]*const res = await fetch\([^;]*\);', "", html, count=1)
    html = html.replace("await res.arrayBuffer()",
                        "Uint8Array.from(atob(__WASM_B64), c => c.charCodeAt(0))")
    token = "__WASM_B64"

if "fetch(" in html:
    sys.exit("make-artifact: a fetch() call survived — the page loads something other than its wasm")
if token not in html:
    sys.exit("make-artifact: failed to inline the wasm blob")

open(out_path, "w", encoding="utf-8").write(html)
print("bytes: %d, modules: %d" % (len(html), len(wasm_paths)))
PY

SIZE="$(wc -c < "$OUT" | tr -d ' ')"
echo "==> wrote $OUT (${SIZE} bytes, self-contained)"
echo
echo "Next:"
echo "  • open it locally:      xdg-open \"$OUT\"  (or drag it into a browser)"
echo "  • host it anywhere:     it needs no server and no network"
echo "  • publish via Claude:   ask Claude Code to publish \"$OUT\" as an artifact"
