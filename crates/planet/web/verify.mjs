// Headless check that the WASM actually renders a planet (no browser needed).
// Usage: node web/verify.mjs
import { readFileSync } from "node:fs";

const bytes = readFileSync(new URL("./planet.wasm", import.meta.url));
const { instance } = await WebAssembly.instantiate(bytes, {});
const { memory, alloc, render, type_count } = instance.exports;

// --- WebGPU path: the demo's shader contract ------------------------------
// index.html and planet.wgsl each declare the per-frame buffer layout, and the
// buffer is untyped floats, so a drifted offset renders a *wrong* planet on the
// GPU rather than failing — and only on machines that have WebGPU, which is
// exactly where nobody notices. Check them against each other here; the shader
// ships inside the wasm, so no browser and no GPU is needed to do it.
{
  const { wgsl_ptr, wgsl_len, gpu_table_len, gpu_table_stride } = instance.exports;
  const wgsl = new TextDecoder().decode(new Uint8Array(memory.buffer, wgsl_ptr(), wgsl_len()));
  const html = readFileSync(new URL("./index.html", import.meta.url), "utf8");

  const fromWgsl = (name) => {
    const m = wgsl.match(new RegExp(`const ${name}: u32 = (\\d+)u;`));
    if (!m) throw new Error(`planet.wgsl is missing const ${name}`);
    return Number(m[1]);
  };
  // index.html declares them all on one `const FR_… = 0, FR_… = 1, …` run.
  const fromHtml = (name) => {
    const m = html.match(new RegExp(`\\b${name}\\s*=\\s*(\\d+)`));
    if (!m) throw new Error(`index.html is missing ${name}`);
    return Number(m[1]);
  };
  const shared = ["FR_SIZE", "FR_ANGLE", "FR_SEED", "FR_TYPE", "FR_PALETTE",
                  "FR_DITHER", "FR_MOONS", "FR_PARAMS", "FR_STRIDE"];
  for (const name of shared) {
    const a = fromWgsl(name), b = fromHtml(name);
    if (a !== b) throw new Error(`${name}: planet.wgsl says ${a}, index.html says ${b}`);
  }
  // The 13 slider params have to fit between FR_PARAMS and the end of the row.
  const need = fromWgsl("FR_PARAMS") + instance.exports.num_params();
  if (need > fromWgsl("FR_STRIDE")) {
    throw new Error(`frame buffer is ${fromWgsl("FR_STRIDE")} floats, layout needs ${need}`);
  }
  // The type table the shader indexes must be a whole number of rows.
  if (gpu_table_len() !== type_count() * gpu_table_stride()) {
    throw new Error("gpu table length is not type_count * stride");
  }
  console.log(`webgpu contract ok (frame ${fromWgsl("FR_STRIDE")} floats, ` +
              `table ${type_count()}x${gpu_table_stride()})`);
}

const SIZE = 64;
const nTypes = type_count();
console.log(`type_count = ${nTypes}`);
// Mirrors planet_core::TYPES, which NAMES in index.html is index-aligned with.
// Pinned rather than bounded: a new archetype has to move all three, and a
// mismatch here is the cheapest place to notice the demo's list went stale.
const TYPE_COUNT = 26;
if (nTypes !== TYPE_COUNT) throw new Error(`expected ${TYPE_COUNT} types (planet_core::TYPES), got ${nTypes}`);

const len = SIZE * SIZE * 4;
const ptr = alloc(len);
render(ptr, SIZE, 0 /*terran*/, 1 /*seed*/, 0.7 /*angle*/);

const buf = new Uint8Array(memory.buffer, ptr, len);

// Deep space is planet_core::star_bg's [9,8,20] — but that is the colour going
// *into* `finalize`, which ordered-dithers every pixel to 22 levels
// (dither_core::quant at the house strength 0.7) before it reaches the buffer.
// The Bayer bias is under half a level, so each channel arrives on one of the
// two steps bracketing the source and never on the raw value: comparing against
// [9,8,20] literally matched nothing, counted the whole frame as planet, and
// left the coverage check below unable to fail.
const LEVELS = 22, DITHER = 0.7;                  // planet_core::finalize
const step = (k) => Math.floor(Math.max(k, 0) / LEVELS * 255);
const spaceSteps = (c) => {
  const f = c / 255 * LEVELS, bias = 0.5 * DITHER; // |dither_core::bayer| < 0.5
  return [step(Math.round(f - bias)), step(Math.round(f + bias))];
};
const SPACE = [9, 8, 20].map(spaceSteps);         // planet_core::star_bg
const isSpace = (r, g, b) =>
  SPACE[0].includes(r) && SPACE[1].includes(g) && SPACE[2].includes(b);

// buffer must be non-empty and contain non-background pixels.
let nonBg = 0;
let allZero = true;
for (let i = 0; i < len; i += 4) {
  const r = buf[i], g = buf[i + 1], b = buf[i + 2], a = buf[i + 3];
  if (r || g || b || a) allZero = false;
  if (!isSpace(r, g, b)) nonBg++; // planet disc, ring or starfield speck
}
const total = SIZE * SIZE;
console.log(`non-background pixels: ${nonBg}/${total}`);
if (allZero) throw new Error("buffer is all zero — render did nothing");
// The hero disc is 0.375*size across (planet_core::render_ct), so a full-size
// world covers ~44% and even ringed_giant — the smallest at radius_scale 0.50 —
// clears 20%. 10% is the floor every type has room above; an empty starfield
// lands near 0.
if (nonBg < total * 0.1) throw new Error("too few non-background pixels — no planet drawn");

// Render a second, different type/seed and confirm it differs from the first.
const ptr2 = alloc(len);
render(ptr2, SIZE, 10 /*lava*/, 99, 2.0);
const buf2 = new Uint8Array(memory.buffer, ptr2, len);
let diff = 0;
for (let i = 0; i < len; i++) if (buf[i] !== buf2[i]) diff++;
console.log(`bytes differing between type 0 and type 10: ${diff}/${len}`);
if (diff === 0) throw new Error("two different types produced identical output");

console.log("PASS: wasm renders distinct, non-empty planets.");
