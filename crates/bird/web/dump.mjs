import { readFileSync, writeFileSync } from 'fs';
const { instance } = await WebAssembly.instantiate(readFileSync(new URL('./bird.wasm', import.meta.url)), {});
const w = instance.exports;
const size = 150, len = size*size*4;
const ptr = w.alloc(len);
w.render(ptr, size, 9001, 0.2, 0.5);
writeFileSync('/tmp/alienframe.raw', Buffer.from(new Uint8Array(w.memory.buffer, ptr, len)));
console.log('wrote', size, 'x', size);
