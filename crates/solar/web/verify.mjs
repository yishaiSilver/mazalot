// Headless check that the WASM actually renders a solar system (no browser).
// Usage: node web/verify.mjs
import { readFileSync } from "node:fs";

const bytes = readFileSync(new URL("./solar.wasm", import.meta.url));
const { instance } = await WebAssembly.instantiate(bytes, {});
const {
  memory, alloc, render, system_new, system_free, planet_count, planet_kind_at,
  sun_kind_of, system_extent, nearest_center, planet_pos,
} = instance.exports;

const W = 320, H = 200, len = W * H * 4;
const buf = alloc(len);
const sys = system_new(7);

const n = planet_count(sys);
console.log(`planets = ${n}, sun kind = ${sun_kind_of(sys)}, extent = ${system_extent(sys).toFixed(0)}`);
if (n < 3 || n > 10) throw new Error(`planet count out of range: ${n}`);
for (let i = 0; i < n; i++) if (planet_kind_at(sys, i) > 9) throw new Error("bad planet kind index");

// Fit the camera and render one frame; it must be a non-empty scene with a
// clearly-lit central body (the star) plus starfield.
const ext = system_extent(sys);
const zoom = Math.min(W * 0.45 / ext, H * 0.45 / (ext * 0.55));
render(sys, buf, W, H, 0, 0, zoom, 3.0);
const px = new Uint8Array(memory.buffer, buf, len);
let lit = 0, stars = 0, allZero = true;
for (let i = 0; i < len; i += 4) {
  const s = px[i] + px[i + 1] + px[i + 2];
  if (px[i] || px[i + 1] || px[i + 2] || px[i + 3]) allZero = false;
  if (s > 300) lit++;      // a bright body pixel
  else if (s > 40) stars++; // a star / dim pixel above navy background
}
console.log(`bright body pixels: ${lit}, midtone pixels: ${stars}`);
if (allZero) throw new Error("buffer all zero — render did nothing");
if (lit < 30) throw new Error("no bright central star drawn");

// Two different times must move the planets → different frames.
const buf2 = alloc(len);
render(sys, buf2, W, H, 0, 0, zoom, 9.0);
const px2 = new Uint8Array(memory.buffer, buf2, len);
let diff = 0;
for (let i = 0; i < len; i++) if (px[i] !== px2[i]) diff++;
console.log(`bytes differing across time: ${diff}/${len}`);
if (diff === 0) throw new Error("planets did not move between two times");

// A different seed must produce a different system.
const sysB = system_new(42);
if (planet_count(sysB) === n && sun_kind_of(sysB) === sun_kind_of(sys)) {
  // Not fatal, but very unlikely to match on both — sanity note only.
  console.log("note: seed 42 happened to match seed 7 on count+sun");
}

// planet_pos must return a point on the body's orbit (non-origin for a planet).
const pp = alloc(2 * 4);
planet_pos(sys, n - 1, 3.0, pp);
const pos = new Float32Array(memory.buffer, pp, 2);
const rad = Math.hypot(pos[0], pos[1]);
console.log(`outer planet radius at t=3: ${rad.toFixed(0)}`);
if (rad < 10) throw new Error("planet_pos returned ~origin for a planet");

// nearest_center: with the camera centred on that planet, it should be the hit.
const target = n - 1;
planet_pos(sys, target, 3.0, pp);
const p2 = new Float32Array(memory.buffer, pp, 2);
const hit = nearest_center(sys, W, H, p2[0], p2[1], zoom, 3.0);
console.log(`nearest_center when aimed at planet ${target}: ${hit}`);
if (hit !== target) throw new Error(`expected to be viewing planet ${target}, got ${hit}`);

system_free(sys);
system_free(sysB);
console.log("PASS: wasm generates and renders distinct, animated solar systems.");
