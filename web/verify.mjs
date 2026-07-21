// Headless check that the WASM actually renders a planet (no browser needed).
// Usage: node web/verify.mjs
import { readFileSync } from "node:fs";

const bytes = readFileSync(new URL("./planet.wasm", import.meta.url));
const { instance } = await WebAssembly.instantiate(bytes, {});
const { memory, alloc, render, type_count } = instance.exports;

const SIZE = 64;
const nTypes = type_count();
console.log(`type_count = ${nTypes}`);
if (nTypes !== 26) throw new Error(`expected 26 types, got ${nTypes}`);

const len = SIZE * SIZE * 4;
const ptr = alloc(len);
render(ptr, SIZE, 0 /*terran*/, 1 /*seed*/, 0.7 /*angle*/);

const buf = new Uint8Array(memory.buffer, ptr, len);

// buffer must be non-empty and contain non-background pixels.
let nonBg = 0;
let allZero = true;
for (let i = 0; i < len; i += 4) {
  const r = buf[i], g = buf[i + 1], b = buf[i + 2], a = buf[i + 3];
  if (r || g || b || a) allZero = false;
  // space color is [9,8,20]; count anything clearly different as "planet/star"
  if (!(r === 9 && g === 8 && b === 20)) nonBg++;
}
const total = SIZE * SIZE;
console.log(`non-background pixels: ${nonBg}/${total}`);
if (allZero) throw new Error("buffer is all zero — render did nothing");
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
