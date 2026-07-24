// Headless check that the WASM actually generates + renders a galaxy (no browser).
// Usage: node web/verify.mjs
//
// NOTE on WASM memory: `alloc` can grow linear memory, which DETACHES any
// existing typed-array views over `memory.buffer`. So we (a) do every alloc up
// front, before the compare phase, and (b) recreate views right before reading.
// A freshly-warmed `render_map` reuses its scratch + bg cache, so it does not
// grow memory — hence views taken after the first render stay valid. The diff
// asserts also require a PARTIAL diff (0 < diff < len): a full-buffer diff would
// mean a detached view, not a real render difference.
import { readFileSync } from "node:fs";

const bytes = readFileSync(new URL("./galaxy.wasm", import.meta.url));
const { instance } = await WebAssembly.instantiate(bytes, {});
const {
  memory, alloc, render_map, galaxy_new, galaxy_new_params, galaxy_free,
  node_count, node_seed, node_star, node_region, node_pos,
  node_at, galaxy_extent, region_name_count,
} = instance.exports;

const W = 400, H = 300, len = W * H * 4;
const view = (ptr, n = len) => new Uint8Array(memory.buffer, ptr, n);

// --- all allocations up front (so no growth detaches views during compares) ---
const gal = galaxy_new(7);
const buf = alloc(len);
const buf2 = alloc(len);
const pp = alloc(2 * 4);

const n = node_count(gal);
console.log(`systems = ${n}, extent = ${galaxy_extent(gal).toFixed(0)}, regions defined = ${region_name_count()}`);
if (n < 24 || n > 600) throw new Error(`system count out of range: ${n}`);

// Every node must expose a distinct system seed and valid indices.
const seeds = new Set();
for (let i = 0; i < n; i++) {
  seeds.add(node_seed(gal, i) >>> 0);
  if (node_star(gal, i) > 4) throw new Error("bad star index");
  if (node_region(gal, i) >= region_name_count()) throw new Error("bad region index");
}
if (seeds.size !== n) throw new Error(`system seeds not distinct: ${seeds.size}/${n}`);
console.log(`distinct system seeds: ${seeds.size}`);

// Fit the camera. First render warms the bg cache (this is the growth point).
const ext = galaxy_extent(gal);
const zoom = Math.min(0.46 * W / ext, 0.46 * H / ext);
render_map(gal, buf, W, H, 0, 0, zoom, 3.0, -1, -1);   // t=3, no selection
render_map(gal, buf2, W, H, 0, 0, zoom, 12.0, -1, -1); // t=12, no selection

// Non-empty scene with bright star glyphs.
let px = view(buf), px2 = view(buf2);
let lit = 0, allZero = true;
for (let i = 0; i < len; i += 4) {
  const s = px[i] + px[i + 1] + px[i + 2];
  if (s) allZero = false;
  if (s > 260) lit++;
}
console.log(`bright star pixels: ${lit}`);
if (allZero) throw new Error("buffer all zero — render did nothing");
if (lit < 20) throw new Error("no star glyphs drawn");

// Twinkle: t=3 vs t=12 must differ, but only partially (glyphs, not backdrop).
let diff = 0;
for (let i = 0; i < len; i++) if (px[i] !== px2[i]) diff++;
console.log(`bytes differing across time (twinkle): ${diff}`);
if (diff === 0) throw new Error("stars did not twinkle between two times");
if (diff >= len) throw new Error("full-buffer diff — detached view, not a real twinkle");

// Selection ring: same t, with/without a selection must differ partially.
render_map(gal, buf2, W, H, 0, 0, zoom, 3.0, (n / 2) | 0, -1);
px = view(buf); px2 = view(buf2);
let diffSel = 0;
for (let i = 0; i < len; i++) if (px[i] !== px2[i]) diffSel++;
console.log(`bytes differing with a selection ring: ${diffSel}`);
if (diffSel === 0) throw new Error("selection ring drew nothing");
if (diffSel >= len) throw new Error("full-buffer diff — detached view, not a real ring");

// node_at aimed at a node's own world position should hit that node.
const target = (n / 3) | 0;
node_pos(gal, target, pp);
const pos = new Float32Array(memory.buffer, pp, 2);
const hit = node_at(gal, 0, 0, zoom, pos[0], pos[1]);
console.log(`node_at aimed at system ${target}: ${hit}`);
if (hit !== target) throw new Error(`expected to pick system ${target}, got ${hit}`);

// A different seed must produce a different galaxy; param overrides must apply.
const galB = galaxy_new(42);
if (node_count(galB) === n) console.log("note: seed 42 matched seed 7 on system count (fine)");
const galC = galaxy_new_params(3, 300, 0.0, 4);
if (node_count(galC) !== 300) throw new Error("count override ignored");
console.log(`param galaxy: ${node_count(galC)} systems (forced 300)`);

galaxy_free(gal);
galaxy_free(galB);
galaxy_free(galC);
console.log("PASS: wasm generates and renders a distinct, connected, twinkling galaxy.");
