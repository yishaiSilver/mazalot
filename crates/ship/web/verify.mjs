// Headless check that the WASM actually generates and renders ships (no
// browser). Exercises exactly the exports index.html calls, so a signature
// drift between lib.rs, wasm.rs and the demo page fails here first.
// Usage: node web/verify.mjs
import { readFileSync } from "node:fs";

const bytes = readFileSync(new URL("./ship.wasm", import.meta.url));
const { instance } = await WebAssembly.instantiate(bytes, {});
const x = instance.exports;
const dec = new TextDecoder();
const str = (p, n) => dec.decode(new Uint8Array(x.memory.buffer, p, n));

// -- the static tables -------------------------------------------------------
const NC = x.class_count(), NR = x.role_count(), NL = x.livery_count(), NP = x.num_params();
console.log(`classes = ${NC}, roles = ${NR}, liveries = ${NL}, params = ${NP}`);
if (NC < 40) throw new Error(`expected a large class table, got ${NC}`);
if (NR < 4 || NL < 4 || NP < 4) throw new Error("a lookup table came back empty");

const names = [];
const perRole = new Array(NR).fill(0);
for (let i = 0; i < NC; i++) {
  const n = str(x.class_name_ptr(i), x.class_name_len(i));
  if (!n.length) throw new Error(`class ${i} has an empty name`);
  if (names.includes(n)) throw new Error(`duplicate class name: ${n}`);
  names.push(n);
  const r = x.class_role(i);
  if (r >= NR) throw new Error(`class ${n} has role ${r} >= ${NR}`);
  perRole[r]++;
  if (!(x.class_length_m(i) > 0)) throw new Error(`class ${n} has no length`);
}
for (let r = 0; r < NR; r++) {
  const rn = str(x.role_name_ptr(r), x.role_name_len(r));
  console.log(`  ${rn.padStart(11)}: ${perRole[r]}`);
  if (perRole[r] === 0) throw new Error(`role ${rn} is empty`);
}

// -- every class must build and render ---------------------------------------
const W = 128, H = 168, len = W * H * 4;
const buf = x.alloc(len);
const fit = x.alloc(2 * 4);
const nameBuf = x.alloc(96);
let widest = 0, thinnest = 1e9;

for (let i = 0; i < NC; i++) {
  const s = x.ship_new(i, 1000 + i * 7);
  if (!s) throw new Error(`ship_new returned null for class ${names[i]}`);
  if (x.ship_class(s) !== i) throw new Error(`ship_class mismatch for ${names[i]}`);
  if (!(x.ship_part_count(s) > 0)) throw new Error(`${names[i]} assembled zero parts`);
  const a = x.ship_aspect(s);
  if (!(a > 0.02 && a < 8)) throw new Error(`${names[i]} has a wild aspect: ${a}`);
  widest = Math.max(widest, a); thinnest = Math.min(thinnest, a);

  const dn = x.ship_designation(s, nameBuf, 96);
  if (dn === 0) throw new Error(`${names[i]} produced no designation`);

  // NB: a render can GROW wasm memory (the first opaque-backdrop render bakes
  // a frame-sized cache), which DETACHES any live typed-array view. So copy the
  // fit into plain numbers straight away and re-view buffers after a render.
  x.ship_fit_with_plume(s, W, H, 0.26, fit);
  const [zoom, pan] = new Float32Array(x.memory.buffer, fit, 2);
  if (!(zoom > 0)) throw new Error(`${names[i]} fit gave a non-positive zoom`);
  // render(ship, buf, w, h, zoom, heading, pan_x, pan_y, thrust, dither, stars, t)
  x.render(s, buf, W, H, zoom, 0, 0, pan, 1.0, 0.7, 0, 0.5);
  const px = new Uint8Array(x.memory.buffer, buf, len);
  let lit = 0;
  for (let k = 3; k < len; k += 4) if (px[k] > 8) lit++;
  const frac = lit / (W * H);
  if (frac < 0.01) throw new Error(`${names[i]}: only ${(frac * 100).toFixed(2)}% covered`);
  if (frac > 0.95) throw new Error(`${names[i]}: ${(frac * 100).toFixed(2)}% covered — bad fit`);
  x.ship_free(s);
}
console.log(`all ${NC} classes assembled + rendered (aspect ${thinnest.toFixed(2)}..${widest.toFixed(2)})`);

// -- the same seed must rebuild the same pixels ------------------------------
const buf2 = x.alloc(len);
const a1 = x.ship_new(3, 4242), a2 = x.ship_new(3, 4242);
x.ship_fit_with_plume(a1, W, H, 0.26, fit);
const [z2, pan2] = new Float32Array(x.memory.buffer, fit, 2);
const countDiff = () => {
  const u = new Uint8Array(x.memory.buffer, buf, len);
  const v = new Uint8Array(x.memory.buffer, buf2, len);
  let d = 0;
  for (let i = 0; i < len; i++) if (u[i] !== v[i]) d++;
  return d;
};
x.render(a1, buf, W, H, z2, 0, 0, pan2, 1.0, 0.7, 0.6, 0.5);
x.render(a2, buf2, W, H, z2, 0, 0, pan2, 1.0, 0.7, 0.6, 0.5);
if (countDiff() !== 0) throw new Error("same seed rendered differently");
console.log("same seed -> identical pixels");

// -- time must animate the plume ---------------------------------------------
x.render(a2, buf2, W, H, z2, 0, 0, pan2, 1.0, 0.7, 0.6, 4.5);
let diff = countDiff();
console.log(`bytes differing across time: ${diff}/${len}`);
if (diff === 0) throw new Error("nothing animated between two times");

// -- heading must actually rotate the hull -----------------------------------
x.render(a1, buf, W, H, x.ship_fit_zoom_spin(a1, W, H), 0, 0, 0, 1.0, 0.7, 0.6, 0.5);
x.render(a2, buf2, W, H, x.ship_fit_zoom_spin(a2, W, H), Math.PI / 2, 0, 0, 1.0, 0.7, 0.6, 0.5);
if (countDiff() < len * 0.05) throw new Error("heading did not rotate the hull");
console.log("heading rotates the hull");
x.ship_free(a1); x.ship_free(a2);

// -- slider params must change the hull --------------------------------------
const pp = x.alloc(NP * 4);
const pv = new Float32Array(x.memory.buffer, pp, NP);
for (let k = 0; k < NP; k++) pv[k] = x.param(20, k);
const base = x.ship_new_params(20, 99, pp, NP);
const baseParts = x.ship_part_count(base);
pv[3] = Math.min(20, x.param(20, 3) + 8); // more turrets
const more = x.ship_new_params(20, 99, pp, NP);
console.log(`turret slider: ${baseParts} parts -> ${x.ship_part_count(more)} parts`);
if (x.ship_part_count(more) <= baseParts) throw new Error("turret slider did not add parts");
x.ship_free(base); x.ship_free(more);

console.log("PASS: wasm generates and renders the full class table.");
