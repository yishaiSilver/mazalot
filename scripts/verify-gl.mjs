// verify-gl.mjs — diff the WebGL2 planet shader against the WebAssembly one.
//
// `crates/planet-core/src/shader.glsl` is a second implementation of the pixel
// loop in lib.rs. `gl_uniforms()` keeps the type table and every seeded constant
// on the Rust side, and `glsl_slot_indices_match_the_rust` pins the wire format
// at `cargo test` time — but nothing checks the *shading* except comparing
// pixels, which is what this does.
//
//   node scripts/verify-gl.mjs [--size 96] [--types all] [--verbose]
//
// It runs both renderers inside headless Chromium, whose WebGL2 is ANGLE over
// SwiftShader — a software rasterizer, so this checks CORRECTNESS ONLY. It says
// nothing about GPU speed, and timing it would measure the CPU.
//
// Expect a small residue rather than zero. `hash3` and `value_noise` are u32
// integer math and port exactly, but the shading around them runs `sin`, `exp`,
// `pow` and `sqrt`, which ANGLE is free to round differently. A pixel then lands
// on 22 quantization levels, so a difference of 1e-7 before the quantizer
// becomes a whole level (12/255) after it. The number that matters is therefore
// not "how many pixels differ" but "how many differ by MORE than one level" —
// those cannot be a threshold flip and are real disagreements.

import fs from "fs";
import path from "path";
import { execSync } from "child_process";
import { fileURLToPath, pathToFileURL } from "url";

const ROOT = path.resolve(path.dirname(fileURLToPath(import.meta.url)), "..");

// Playwright is usually installed globally rather than into this repo (which has
// no package.json), and a global install is not on the module resolution path.
async function loadPlaywright() {
  for (const spec of ["playwright", "playwright-core"]) {
    try { return await import(spec); } catch { /* not local — try the next */ }
  }
  try {
    const root = execSync("npm root -g", { stdio: ["ignore", "pipe", "ignore"] }).toString().trim();
    return await import(pathToFileURL(path.join(root, "playwright", "index.mjs")).href);
  } catch {
    console.error("verify-gl: playwright not found — `npm i -g playwright` (Chromium is what runs the shaders)");
    process.exit(2);
  }
}
const { chromium } = await loadPlaywright();

const argv = process.argv.slice(2);
const flag = (name, dflt) => {
  const i = argv.indexOf(`--${name}`);
  return i === -1 ? dflt : argv[i + 1];
};
const SIZE = parseInt(flag("size", "96"));
const ALL = flag("types", "") === "all";
const VERBOSE = argv.includes("--verbose");

const wasmPath = path.join(ROOT, "crates/planet/web/planet.wasm");
if (!fs.existsSync(wasmPath)) {
  console.error(`no wasm at ${wasmPath} — build it first:
  cargo build -p planet --target wasm32-unknown-unknown --release --no-default-features
  cp target/wasm32-unknown-unknown/release/planet.wasm crates/planet/web/planet.wasm`);
  process.exit(2);
}
const bytes = [...fs.readFileSync(wasmPath)];

// SwiftShader, because this container has no GPU. `--enable-unsafe-swiftshader`
// is what lets a headless Chromium hand out a WebGL2 context without one.
const browser = await chromium.launch({
  args: ["--enable-unsafe-swiftshader", "--use-gl=angle", "--use-angle=swiftshader"],
});
const page = await browser.newPage();
page.on("console", m => { if (VERBOSE || m.type() === "error") console.log(`  [page] ${m.text()}`); });
await page.setContent("<!doctype html><meta charset=utf-8><body></body>");

const out = await page.evaluate(async ({ bytes, size, all }) => {
  const wasm = (await WebAssembly.instantiate(Uint8Array.from(bytes), {})).instance.exports;
  const NUM = wasm.num_params();
  const FEAT = wasm.feat_all();
  const nTypes = wasm.type_count();

  const c = document.createElement("canvas");
  c.width = c.height = size;
  const gl = c.getContext("webgl2", {
    alpha: false, antialias: false, depth: false, stencil: false,
    premultipliedAlpha: false, preserveDrawingBuffer: true,
  });
  if (!gl) return { fatal: "no WebGL2 context" };

  const VS = `#version 300 es
void main() {
  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
  gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}`;
  const FS = new TextDecoder().decode(
    new Uint8Array(wasm.memory.buffer, wasm.gl_shader_ptr(), wasm.gl_shader_len()));

  const mk = (t, src, what) => {
    const s = gl.createShader(t);
    gl.shaderSource(s, src); gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(`${what}: ${gl.getShaderInfoLog(s)}`);
    return s;
  };
  let prog;
  try {
    prog = gl.createProgram();
    gl.attachShader(prog, mk(gl.VERTEX_SHADER, VS, "vertex"));
    gl.attachShader(prog, mk(gl.FRAGMENT_SHADER, FS, "fragment"));
    gl.linkProgram(prog);
    if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) throw new Error(`link: ${gl.getProgramInfoLog(prog)}`);
  } catch (e) {
    return { fatal: String(e.message || e) };
  }
  const loc = {
    U: gl.getUniformLocation(prog, "U"),
    seed: gl.getUniformLocation(prog, "u_seed"),
    feat: gl.getUniformLocation(prog, "u_feat"),
    size: gl.getUniformLocation(prog, "u_size"),
    palette: gl.getUniformLocation(prog, "u_palette"),
  };

  const n = size * size * 4;
  const bufPtr = wasm.alloc(n);
  const pp = wasm.alloc(NUM * 4);
  const uLen = wasm.gl_uniforms_len();
  const uPtr = wasm.alloc(uLen * 4);
  const glPx = new Uint8Array(n);
  const flipped = new Uint8Array(n);
  const renderer = gl.getParameter(gl.RENDERER);

  const rows = [];
  const types = all ? [...Array(nTypes).keys()] : [0, 1, 6, 7, 8, 10, 18, 20, 24];
  const angles = [0.0, 0.9, 2.7, 4.4];

  for (const t of types) {
    // Type defaults, written once — both renderers read the same array.
    const pv = new Float32Array(wasm.memory.buffer, pp, NUM);
    for (let i = 0; i < NUM; i++) pv[i] = wasm.param(t, i);

    let diff = 0, big = 0, worst = 0, sum = 0, total = 0;
    for (const ang of angles) {
      for (const seed of [1, 424242]) {
        wasm.render_features(bufPtr, size, t, seed, ang, pp, 0, 0.7, 1, FEAT);
        const cpu = new Uint8Array(wasm.memory.buffer, bufPtr, n).slice();

        wasm.gl_uniforms(uPtr, size, t, seed, ang, pp, 0, 0.7, 1, FEAT);
        gl.viewport(0, 0, size, size);
        gl.useProgram(prog);
        gl.uniform1fv(loc.U, new Float32Array(wasm.memory.buffer, uPtr, uLen));
        gl.uniform1ui(loc.seed, seed >>> 0);
        gl.uniform1ui(loc.feat, FEAT >>> 0);
        gl.uniform1i(loc.size, size);
        gl.uniform1i(loc.palette, 0);
        gl.drawArrays(gl.TRIANGLES, 0, 3);
        gl.readPixels(0, 0, size, size, gl.RGBA, gl.UNSIGNED_BYTE, glPx);
        // readPixels is bottom-up; the frame is top-down.
        for (let y = 0; y < size; y++) {
          flipped.set(glPx.subarray((size - 1 - y) * size * 4, (size - y) * size * 4), y * size * 4);
        }

        for (let i = 0; i < n; i += 4) {
          let d = 0;
          for (let k = 0; k < 3; k++) d = Math.max(d, Math.abs(cpu[i + k] - flipped[i + k]));
          total++;
          if (d > 0) { diff++; sum += d; }
          // One 22-level step is 255/22 = 11.6, so anything past 12 cannot be a
          // pixel that merely fell on the other side of a quantizer threshold.
          if (d > 12) big++;
          worst = Math.max(worst, d);
        }
      }
    }
    rows.push({ t, name: wasm.type_count && t, diff, big, worst, mean: diff ? sum / diff : 0, total });
  }
  return { renderer, rows, nTypes };
}, { bytes, size: SIZE, all: ALL });

await browser.close();

if (out.fatal) {
  console.error(`verify-gl: ${out.fatal}`);
  process.exit(1);
}

const NAMES = [
  "terran", "ocean", "archipelago", "desert", "swamp", "iron", "ice", "barren",
  "gas_giant", "ice_giant", "lava", "fungal", "savanna", "gaia", "tundra", "alpine",
  "obsidian", "chrome", "moon", "storm_giant", "ringed_giant", "molten_sea",
  "radioactive", "crystal", "toxic", "storm_shroud",
];

console.log(`renderer: ${out.renderer}`);
console.log(`${SIZE}x${SIZE}, 4 angles x 2 seeds per type\n`);
console.log(`${"type".padEnd(14)}${"differ".padStart(9)}${">1 level".padStart(10)}${"max".padStart(6)}${"mean|d|".padStart(9)}`);

let worstBig = 0, worstMax = 0;
for (const r of out.rows) {
  const pct = v => `${(100 * v / r.total).toFixed(2)}%`;
  worstBig = Math.max(worstBig, r.big / r.total);
  worstMax = Math.max(worstMax, r.worst);
  console.log(
    `${(NAMES[r.t] ?? `type ${r.t}`).padEnd(14)}${pct(r.diff).padStart(9)}` +
    `${pct(r.big).padStart(10)}${String(r.worst).padStart(6)}${r.mean.toFixed(2).padStart(9)}`);
}

// A threshold flip is expected; a structural disagreement is not. 0.5% of pixels
// past a whole quantization level is well above the flip rate and well below
// anything a wrong uniform slot or a mistyped constant would produce — those
// wreck a whole layer, not a fringe.
const LIMIT = 0.005;
console.log(`\nworst per-type rate past one quantization level: ${(100 * worstBig).toFixed(2)}%  (max delta ${worstMax}/255)`);
if (worstBig > LIMIT) {
  console.error(`FAIL: over ${(100 * LIMIT).toFixed(1)}% — the two shaders disagree structurally, not just at the quantizer.`);
  process.exit(1);
}
console.log("OK — the difference is confined to quantizer threshold flips.");
