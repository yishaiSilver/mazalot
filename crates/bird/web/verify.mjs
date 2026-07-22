import { readFileSync } from 'fs';
const bytes = readFileSync(new URL('./bird.wasm', import.meta.url));
const { instance } = await WebAssembly.instantiate(bytes, {});
const w = instance.exports;
const size = 120, len = size * size * 4, bg = [10, 9, 16];
const ptr = w.alloc(len);
console.log('native_grid =', w.native_grid());
// render a few frames/seeds; ensure a creature actually draws and animates
let frameCounts = [];
for (const ph of [0.0, 0.25, 0.5, 0.75]) {
  w.render(ptr, size, 9001, ph, 0.5);
  const mem = new Uint8Array(w.memory.buffer, ptr, len);
  let n = 0;
  for (let i = 0; i < len; i += 4)
    if (mem[i] !== bg[0] || mem[i+1] !== bg[1] || mem[i+2] !== bg[2]) n++;
  frameCounts.push(n);
}
console.log('non-bg pixels per phase:', frameCounts);
const drew = frameCounts.every(n => n > 80);
const moved = new Set(frameCounts).size > 1; // animation changes the silhouette
console.log(drew ? 'OK: creature renders' : 'FAIL: nothing drew');
console.log(moved ? 'OK: it animates' : 'note: identical across phases');
process.exit(drew ? 0 : 1);
