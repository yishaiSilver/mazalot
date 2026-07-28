#!/usr/bin/env bash
#
# make-parallel-probe.sh — build the parallel capability probe: a SINGLE
# self-contained HTML file that reports what a host lets a page do with more
# than one core, and measures what that is actually worth here.
#
# The demos render every frame in one wasm instance on the main thread, so they
# use exactly one core no matter how many the machine has. Before writing a
# worker pool it is worth knowing, on the host you deploy to:
#
#   * are blob-URL workers allowed?  A single self-contained file has no other
#     way off the main thread, and a strict CSP can forbid them.
#   * is the page cross-origin isolated?  Without COOP/COEP there is no
#     SharedArrayBuffer — which rules out a rayon-style shared-memory port but
#     NOT worker-per-region, which uses transferable buffers instead. GitHub
#     Pages cannot set response headers, so it is always in this bucket.
#   * what speedup do the cores really give?  The probe runs the real
#     `planet.wasm` shader, not a synthetic loop, and prints the dispatch
#     overhead beside it — splitting one frame pays that every frame.
#
# Usage:
#   scripts/make-parallel-probe.sh [--out FILE] [--no-build]
#
#   --out FILE    output path (default: dist/parallel-probe.html)
#   --no-build    skip the wasm rebuild; use the committed crates/planet/web/planet.wasm
set -euo pipefail

cd "$(dirname "$0")/.."
out="dist/parallel-probe.html"
build=1
while [ $# -gt 0 ]; do
  case "$1" in
    --out) out="$2"; shift 2 ;;
    --no-build) build=0; shift ;;
    -h|--help) sed -n '2,25p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *) echo "unknown argument: $1" >&2; exit 1 ;;
  esac
done

wasm="crates/planet/web/planet.wasm"
if [ "$build" = 1 ]; then
  # From the repo root, so .cargo/config.toml applies — simd128 is required.
  cargo build -q -p planet --target wasm32-unknown-unknown --release --no-default-features
  cp target/wasm32-unknown-unknown/release/planet.wasm "$wasm"
fi

mkdir -p "$(dirname "$out")"
python3 - "$wasm" "$out" <<'PY'
import base64, sys
wasm, out = sys.argv[1], sys.argv[2]
tpl = open('scripts/parallel-probe.template.html').read()
b64 = base64.b64encode(open(wasm, 'rb').read()).decode()
assert '__WASM_B64__' in tpl, 'template lost its wasm placeholder'
open(out, 'w').write(tpl.replace('__WASM_B64__', b64))
PY

printf '\nWrote %s (%s bytes)\n' "$out" "$(wc -c < "$out")"
printf '  • open it locally, or drop it on any static host (GitHub Pages included)\n'
printf '  • it needs no network and no special headers\n'
