#!/usr/bin/env bash
#
# build-wasm.sh — rebuild the demo WebAssembly modules and refresh the copies
# committed under crates/<crate>/web/.
#
# Those committed .wasm files are what the repo-root dev server and
# `make-artifact.sh --no-build` actually serve, and they go stale the moment a
# render path changes — the failure is silent, because a stale module still
# loads and still draws, just not what the Rust says. This is the one command
# that resyncs them.
#
# Usage:
#   scripts/build-wasm.sh [crate...]     (default: every crate with a web/ dir)
#
# Examples:
#   scripts/build-wasm.sh                # all of them
#   scripts/build-wasm.sh solar planet   # just these two
#
set -euo pipefail

die() { printf 'build-wasm: %s\n' "$1" >&2; exit 1; }

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

command -v cargo >/dev/null 2>&1 || die "cargo not found; install Rust"
if ! (rustup target list --installed 2>/dev/null | grep -q wasm32-unknown-unknown); then
  die "wasm target missing — run: rustup target add wasm32-unknown-unknown"
fi

# Default to every crate that ships a browser demo. `character` has none, and
# the *-core libs are rlibs, so neither shows up here.
if [ $# -gt 0 ]; then
  CRATES=("$@")
else
  CRATES=()
  for d in "$ROOT"/crates/*/web/; do
    CRATES+=("$(basename "$(dirname "$d")")")
  done
fi

for crate in "${CRATES[@]}"; do
  [ -d "$ROOT/crates/$crate/web" ] || die "no demo at crates/$crate/web/"
done

# --no-default-features is what keeps `native` (and so image/gif/tokio) out of
# the module; see the feature-gate note in CLAUDE.md.
echo "==> building ${#CRATES[@]} wasm module(s), release, --no-default-features"
for crate in "${CRATES[@]}"; do
  ( cd "$ROOT" && cargo build -q -p "$crate" --target wasm32-unknown-unknown --release --no-default-features )
  src="$ROOT/target/wasm32-unknown-unknown/release/$crate.wasm"
  dst="$ROOT/crates/$crate/web/$crate.wasm"
  [ -f "$src" ] || die "cargo produced no $src"

  if [ -f "$dst" ] && cmp -s "$src" "$dst"; then
    state="unchanged"
  else
    state="UPDATED"
  fi
  cp "$src" "$dst"
  printf '    %-10s %7d bytes  %s\n' "$crate" "$(wc -c < "$dst" | tr -d ' ')" "$state"
done
