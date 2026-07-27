#!/usr/bin/env bash
#
# build-site.sh — assemble the deployable static site into site/.
#
# The repo root doubles as a dev server root (`python3 -m http.server` finds the
# landing page and the demos in place), but it is not a website: it would also
# serve src/, target/, out/ and .git. This produces a clean tree containing only
# what a visitor needs:
#
#   site/
#     index.html            landing page, links rewritten to the flat layout
#     demos/<crate>/
#       index.html          the demo page, wasm URL content-stamped
#       <crate>.wasm
#
# Everything is referenced with relative paths, so the result works both at a
# domain root and under a subpath like /mazalot/ (which is how GitHub Pages
# serves project sites).
#
# Usage:
#   scripts/build-site.sh [--out DIR] [--no-build] [--serve [PORT]]
#
#   --out DIR     output directory (default: site/)
#   --no-build    reuse the committed crates/<crate>/web/<crate>.wasm
#   --serve PORT  serve the result after building (default port 8000)
#
set -euo pipefail

die() { printf 'build-site: %s\n' "$1" >&2; exit 1; }

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

OUT="$ROOT/site"
BUILD=1
SERVE=0
PORT=8000
while [ $# -gt 0 ]; do
  case "$1" in
    --out)      OUT="${2:-}"; [ -n "$OUT" ] || die "--out needs a path"; shift 2 ;;
    --no-build) BUILD=0; shift ;;
    --serve)    SERVE=1
                case "${2:-}" in [0-9]*) PORT="$2"; shift 2 ;; *) shift ;; esac ;;
    -h|--help)  sed -n '2,25p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'; exit 0 ;;
    *)          die "unknown argument: $1" ;;
  esac
done

command -v python3 >/dev/null 2>&1 || die "python3 is required (used to rewrite the pages)"

# Rebuilding is the default because a stale module is a silent failure: it still
# loads and still draws, just not what the Rust says.
if [ "$BUILD" -eq 1 ]; then
  "$ROOT/scripts/build-wasm.sh"
fi

CRATES=()
for d in "$ROOT"/crates/*/web/; do
  CRATES+=("$(basename "$(dirname "$d")")")
done
[ "${#CRATES[@]}" -gt 0 ] || die "no crates/*/web/ demos found"

rm -rf "$OUT"
mkdir -p "$OUT/demos"

echo "==> assembling site at $OUT"
python3 - "$ROOT" "$OUT" "${CRATES[@]}" <<'PY'
import hashlib, pathlib, re, sys

root, out, crates = pathlib.Path(sys.argv[1]), pathlib.Path(sys.argv[2]), sys.argv[3:]

def stamp(data):
    """Short content hash. Changing the wasm changes the URL, so a cached copy
    is never the stale one — which is what lets us drop `cache: "no-store"`."""
    return hashlib.sha256(data).hexdigest()[:12]

stamps = {}
for crate in crates:
    src = root / "crates" / crate / "web"
    dst = out / "demos" / crate
    dst.mkdir(parents=True, exist_ok=True)

    wasm = (src / f"{crate}.wasm").read_bytes()
    (dst / f"{crate}.wasm").write_bytes(wasm)
    stamps[crate] = v = stamp(wasm)

    html = (src / "index.html").read_text(encoding="utf-8")
    # Every demo fetches its module with the same dev-mode line:
    #   const res = await fetch("./<crate>.wasm?v=" + Date.now(), { cache: "no-store" });
    pattern = (r'fetch\(\s*"\./%s\.wasm\?v=" \+ Date\.now\(\),\s*\{\s*cache:\s*"no-store"\s*\}\s*\)'
               % re.escape(crate))
    html, n = re.subn(pattern, 'fetch("./%s.wasm?v=%s")' % (crate, v), html, count=1)
    if n != 1:
        sys.exit(f"build-site: could not stamp the wasm fetch in {crate}/index.html "
                 f"(expected the dev-mode fetch line; did the demo's loader change?)")
    (dst / "index.html").write_text(html, encoding="utf-8")

    # Anything else the demo ships beside its page (verify/dump helpers are dev
    # tools and stay out of the deployed tree).
    for extra in src.iterdir():
        if extra.name in (f"{crate}.wasm", "index.html") or extra.suffix == ".mjs":
            continue
        if extra.is_file():
            (dst / extra.name).write_bytes(extra.read_bytes())

# The landing page points at the crates tree for dev; retarget it at the flat
# layout and stamp its thumbnail fetches. Both lines are marked in index.html.
landing = (root / "index.html").read_text(encoding="utf-8")
landing, n1 = re.subn(r'const DEMO_BASE = "crates/\{crate\}/web";',
                      'const DEMO_BASE = "demos/{crate}";', landing, count=1)
# One stamp for all thumbnails: they are decorative, and refetching them once
# per deploy is the correct behaviour.
site_v = stamp("".join(stamps[c] for c in sorted(stamps)).encode())
landing, n2 = re.subn(r'const WASM_V = "";',
                      'const WASM_V = "?v=%s";' % site_v, landing, count=1)
if n1 != 1 or n2 != 1:
    sys.exit("build-site: could not rewrite index.html — the DEMO_BASE/WASM_V "
             "lines the build depends on are missing or changed")
(out / "index.html").write_text(landing, encoding="utf-8")

# GitHub Pages will not serve a directory that looks like a Jekyll source tree
# unless this is present; harmless everywhere else.
(out / ".nojekyll").write_text("")

for crate in crates:
    page = out / "demos" / crate / "index.html"
    print("    %-10s %7d B wasm  %7d B html  v=%s"
          % (crate, (out / "demos" / crate / f"{crate}.wasm").stat().st_size,
             page.stat().st_size, stamps[crate]))
print("    %-10s %7s      %7d B html  v=%s" % ("(landing)", "-", (out / "index.html").stat().st_size, site_v))
PY

TOTAL="$(du -sh "$OUT" | cut -f1)"
FILES="$(find "$OUT" -type f | wc -l | tr -d ' ')"
echo "==> $OUT: $FILES files, $TOTAL"

if [ "$SERVE" -eq 1 ]; then
  echo "==> serving $OUT at http://localhost:$PORT/  (Ctrl-C to stop)"
  ( cd "$OUT" && python3 -m http.server "$PORT" )
fi
