// Headless check that the WASM actually renders a star (no browser needed).
// Usage: node web/verify.mjs
import { readFileSync } from "node:fs";

const bytes = readFileSync(new URL("./star.wasm", import.meta.url));
const { instance } = await WebAssembly.instantiate(bytes, {});
const { memory, alloc, render, render_params, param, num_params, type_count } = instance.exports;

const SIZE = 80;
const nTypes = type_count();
console.log(`type_count = ${nTypes}`);
if (nTypes !== 8) throw new Error(`expected 8 star types, got ${nTypes}`);

const len = SIZE * SIZE * 4;
const ptr = alloc(len);
render(ptr, SIZE, 2 /*yellow_dwarf*/, 1 /*seed*/, 0.7 /*angle*/);
const buf = new Uint8Array(memory.buffer, ptr, len);

// Buffer must be non-empty and contain a bright disc against black.
let nonBg = 0;
let allZero = true;
for (let i = 0; i < len; i += 4) {
  const r = buf[i], g = buf[i + 1], b = buf[i + 2], a = buf[i + 3];
  if (r || g || b || a) allZero = false;
  if (r + g + b > 24) nonBg++; // clearly-lit (star) pixel, not near-black background
}
const total = SIZE * SIZE;
console.log(`lit pixels: ${nonBg}/${total}`);
if (allZero) throw new Error("buffer is all zero — render did nothing");
if (nonBg < total * 0.1) throw new Error("too few lit pixels — no star drawn");

// param() + render_params() path: overriding params must change the output.
const NUM = num_params();
console.log(`num_params = ${NUM}`);
const pp = alloc(NUM * 4);
const pv = new Float32Array(memory.buffer, pp, NUM);
for (let i = 0; i < NUM; i++) pv[i] = param(2, i); // yellow_dwarf defaults
const ptr2 = alloc(len);
render_params(ptr2, SIZE, 2, 1, 0.7, pp, 0.7, 0.0);
const buf2 = new Uint8Array(memory.buffer, ptr2, len);
// defaults through render_params should closely match plain render()
let same = 0;
for (let i = 0; i < len; i++) if (buf[i] === buf2[i]) same++;
console.log(`render vs render_params(defaults) identical bytes: ${same}/${len}`);
if (same < len * 0.99) throw new Error("render_params with type defaults diverged from render");

// A different type must produce different pixels.
const ptr3 = alloc(len);
render(ptr3, SIZE, 4 /*red_giant*/, 99, 2.0);
const buf3 = new Uint8Array(memory.buffer, ptr3, len);
let diff = 0;
for (let i = 0; i < len; i++) if (buf[i] !== buf3[i]) diff++;
console.log(`bytes differing between type 2 and type 4: ${diff}/${len}`);
if (diff === 0) throw new Error("two different types produced identical output");

console.log("PASS: wasm renders distinct, non-empty stars.");
