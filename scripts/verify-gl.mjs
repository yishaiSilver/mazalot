// verify-gl.mjs — diff the WebGL2 renderers against the WebAssembly ones.
//
// The `.glsl` files under `crates/*/src/` are second implementations of pixel
// loops that live in Rust. `gl_uniforms()` keeps every table and seeded constant
// on the Rust side, and each crate's `glsl_slot_indices_match_the_rust` pins the
// wire format at `cargo test` time — but nothing checks the *shading* except
// comparing pixels, which is what this does.
//
//   node scripts/verify-gl.mjs                       # planet, 9 types, 96px
//   node scripts/verify-gl.mjs --demo all --types all
//   node scripts/verify-gl.mjs --demo solar --size 240
//
// It runs both renderers inside headless Chromium, whose WebGL2 is ANGLE over
// SwiftShader — a software rasterizer, so this checks CORRECTNESS ONLY. It says
// nothing about GPU speed, and timing it would measure the CPU.
//
// Expect a small residue rather than zero. `hash3` and `value_noise` are u32
// integer math and port exactly, but the shading around them runs `sin`, `exp`,
// `pow` and `sqrt`, which ANGLE is free to round differently. A pixel then lands
// on 22 (planet) or 24 (scene) quantization levels, so a difference of 1e-7
// before the quantizer becomes a whole level after it. The number that matters
// is therefore not "how many pixels differ" but "how many differ by MORE than
// one level" — those cannot be a threshold flip and are real disagreements.

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

const argv = process.argv.slice(2);
const flag = (name, dflt) => {
  const i = argv.indexOf(`--${name}`);
  return i === -1 ? dflt : argv[i + 1];
};
const DEMO = flag("demo", "planet");
const SIZE = parseInt(flag("size", DEMO === "solar" ? "220" : "96"));
const ALL = flag("types", "") === "all";
const VERBOSE = argv.includes("--verbose");

function wasmOf(crate) {
  const p = path.join(ROOT, `crates/${crate}/web/${crate}.wasm`);
  if (!fs.existsSync(p)) {
    console.error(`no wasm at ${p} — build it first:
  cargo build -p ${crate} --target wasm32-unknown-unknown --release --no-default-features
  cp target/wasm32-unknown-unknown/release/${crate}.wasm ${p}`);
    process.exit(2);
  }
  return [...fs.readFileSync(p)];
}

// One 22-level step is 255/22 = 11.6 and one 24-level step is 10.6, so anything
// past 12 cannot be a pixel that merely fell on the other side of a quantizer
// threshold.
const LEVEL = 12;
// A rate above this is a structural disagreement — a wrong uniform slot or a
// mistyped constant wrecks a whole layer, not a fringe.
const LIMIT = 0.005;

// ---------------------------------------------------------------------------
// Shared browser-side helpers, stringified into the page.
// ---------------------------------------------------------------------------
// `var`, not `const`: these are injected with a direct `eval` inside the page
// callback, and a `const` there gets its own lexical scope and never reaches the
// caller. This is the one place in the repo where that distinction matters.
const GL_HELPERS = `
  var mkShader = (gl, t, src, what) => {
    const s = gl.createShader(t);
    gl.shaderSource(s, src); gl.compileShader(s);
    if (!gl.getShaderParameter(s, gl.COMPILE_STATUS)) throw new Error(what + ": " + gl.getShaderInfoLog(s));
    return s;
  };
  var mkProgram = (gl, vs, fs, what, unis) => {
    const p = gl.createProgram();
    gl.attachShader(p, mkShader(gl, gl.VERTEX_SHADER, vs, what + " vertex"));
    gl.attachShader(p, mkShader(gl, gl.FRAGMENT_SHADER, fs, what + " fragment"));
    gl.linkProgram(p);
    if (!gl.getProgramParameter(p, gl.LINK_STATUS)) throw new Error(what + " link: " + gl.getProgramInfoLog(p));
    p.loc = {};
    for (const u of unis) p.loc[u] = gl.getUniformLocation(p, u);
    return p;
  };
  // readPixels is bottom-up; every frame in this repo is top-down.
  var flipRows = (src, dst, w, h) => {
    for (let y = 0; y < h; y++) dst.set(src.subarray((h - 1 - y) * w * 4, (h - y) * w * 4), y * w * 4);
  };
  var compare = (cpu, gpu, acc) => {
    for (let i = 0; i < cpu.length; i += 4) {
      let d = 0;
      for (let k = 0; k < 3; k++) d = Math.max(d, Math.abs(cpu[i + k] - gpu[i + k]));
      acc.total++;
      if (d > 0) { acc.diff++; acc.sum += d; }
      if (d > ${LEVEL}) acc.big++;
      acc.worst = Math.max(acc.worst, d);
    }
  };
  var srcOf = (wasm, i) => new TextDecoder().decode(
    new Uint8Array(wasm.memory.buffer, wasm.gl_src_ptr(i), wasm.gl_src_len(i)));
`;

// ---------------------------------------------------------------------------
// planet
// ---------------------------------------------------------------------------
async function checkPlanet(page, bytes, size, all) {
  return page.evaluate(async ({ bytes, size, all, helpers }) => {
    eval(helpers);
    const wasm = (await WebAssembly.instantiate(Uint8Array.from(bytes), {})).instance.exports;
    const NUM = wasm.num_params();
    const FEAT = wasm.feat_all();

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
    let prog;
    try {
      const src = srcOf(wasm, 0) + srcOf(wasm, 1) + srcOf(wasm, 2);
      prog = mkProgram(gl, VS, src, "planet",
        ["U", "u_seed", "u_feat", "u_size", "u_vh", "u_palette"]);
    } catch (e) { return { fatal: String(e.message || e) }; }

    const n = size * size * 4;
    const bufPtr = wasm.alloc(n);
    const pp = wasm.alloc(NUM * 4);
    const uLen = wasm.gl_uniforms_len();
    const uPtr = wasm.alloc(uLen * 4);
    const raw = new Uint8Array(n), flipped = new Uint8Array(n);

    const rows = [];
    const types = all ? [...Array(wasm.type_count()).keys()] : [0, 1, 6, 7, 8, 10, 18, 20, 24];
    for (const t of types) {
      const pv = new Float32Array(wasm.memory.buffer, pp, NUM);
      for (let i = 0; i < NUM; i++) pv[i] = wasm.param(t, i);
      const acc = { total: 0, diff: 0, big: 0, sum: 0, worst: 0 };
      for (const ang of [0.0, 0.9, 2.7, 4.4]) {
        for (const seed of [1, 424242]) {
          wasm.render_features(bufPtr, size, t, seed, ang, pp, 0, 0.7, 1, FEAT);
          const cpu = new Uint8Array(wasm.memory.buffer, bufPtr, n).slice();

          wasm.gl_uniforms(uPtr, size, t, seed, ang, pp, 0, 0.7, 1, FEAT);
          gl.viewport(0, 0, size, size);
          gl.useProgram(prog);
          gl.uniform1fv(prog.loc.U, new Float32Array(wasm.memory.buffer, uPtr, uLen));
          gl.uniform1ui(prog.loc.u_seed, seed >>> 0);
          gl.uniform1ui(prog.loc.u_feat, FEAT >>> 0);
          gl.uniform1i(prog.loc.u_size, size);
          gl.uniform1i(prog.loc.u_vh, size);
          gl.uniform1i(prog.loc.u_palette, 0);
          gl.drawArrays(gl.TRIANGLES, 0, 3);
          gl.readPixels(0, 0, size, size, gl.RGBA, gl.UNSIGNED_BYTE, raw);
          flipRows(raw, flipped, size, size);
          compare(cpu, flipped, acc);
        }
      }
      rows.push({ label: wasm.type_count ? String(t) : String(t), ...acc });
    }
    return { renderer: gl.getParameter(gl.RENDERER), rows };
  }, { bytes, size, all, helpers: GL_HELPERS });
}

// ---------------------------------------------------------------------------
// solar — the whole scene: backdrop, orbit paths, depth-sorted bodies
// ---------------------------------------------------------------------------
async function checkSolar(page, bytes, size) {
  return page.evaluate(async ({ bytes, size, helpers }) => {
    eval(helpers);
    const wasm = (await WebAssembly.instantiate(Uint8Array.from(bytes), {})).instance.exports;
    const W = size, H = Math.round(size * 0.62);

    const c = document.createElement("canvas");
    c.width = W; c.height = H;
    const gl = c.getContext("webgl2", {
      alpha: false, antialias: false, depth: false, stencil: false,
      premultipliedAlpha: false, preserveDrawingBuffer: true,
    });
    if (!gl) return { fatal: "no WebGL2 context" };

    const VS_FULL = `#version 300 es
void main() {
  vec2 p = vec2(float((gl_VertexID << 1) & 2), float(gl_VertexID & 2));
  gl_Position = vec4(p * 2.0 - 1.0, 0.0, 1.0);
}`;
    const VS_QUAD = `#version 300 es
uniform vec4 u_rect; uniform vec2 u_view;
void main() {
  vec2 q = vec2(float(gl_VertexID & 1), float((gl_VertexID >> 1) & 1));
  vec2 p = u_rect.xy + q * u_rect.zw;
  gl_Position = vec4(p.x / u_view.x * 2.0 - 1.0, 1.0 - p.y / u_view.y * 2.0, 0.0, 1.0);
}`;
    const VS_POINT = `#version 300 es
layout(location = 0) in vec2 a_pos; uniform vec2 u_view; uniform float u_size;
void main() {
  gl_PointSize = u_size;
  gl_Position = vec4(a_pos.x / u_view.x * 2.0 - 1.0, 1.0 - a_pos.y / u_view.y * 2.0, 0.0, 1.0);
}`;
    const FS_POINT = `#version 300 es
precision highp float; out vec4 fragColor;
void main() { fragColor = vec4(26.0 / 255.0, 30.0 / 255.0, 40.0 / 255.0, 1.0); }`;

    let progBg, progStar, progPlanet, progOrbit;
    try {
      const pre = srcOf(wasm, 0) + srcOf(wasm, 1);
      progBg = mkProgram(gl, VS_FULL, pre + srcOf(wasm, 2), "backdrop", ["B", "u_skySalt", "u_vh"]);
      progStar = mkProgram(gl, VS_QUAD, pre + srcOf(wasm, 3), "star",
        ["S", "u_size", "u_vh", "u_rect", "u_view"]);
      progPlanet = mkProgram(gl, VS_QUAD, pre + srcOf(wasm, 4), "planet",
        ["U", "u_seed", "u_feat", "u_size", "u_vh", "u_palette", "u_rect", "u_view"]);
      progOrbit = mkProgram(gl, VS_POINT, FS_POINT, "orbit", ["u_view", "u_size"]);
    } catch (e) { return { fatal: String(e.message || e) }; }

    const bgLen = wasm.gl_backdrop_len();
    const stride = wasm.gl_body_stride(), header = wasm.gl_body_header();
    const maxB = wasm.gl_max_bodies(), orbCap = 16 * 220;
    const glFeat = wasm.gl_feat();
    const n = W * H * 4;
    const bufPtr = wasm.alloc(n);
    const bgPtr = wasm.alloc(bgLen * 4);
    const orbPtr = wasm.alloc(orbCap * 2 * 4);
    const bodyPtr = wasm.alloc(maxB * stride * 4);
    const raw = new Uint8Array(n), flipped = new Uint8Array(n);

    const vao = gl.createVertexArray(), vbo = gl.createBuffer();
    gl.bindVertexArray(vao);
    gl.bindBuffer(gl.ARRAY_BUFFER, vbo);
    gl.enableVertexAttribArray(0);
    gl.vertexAttribPointer(0, 2, gl.FLOAT, false, 0, 0);
    gl.bindVertexArray(null);

    // Mirrors `renderGLScene` in crates/solar/web/index.html.
    function drawScene(sys, cx, cy, zoom, bgx, bgy, tO, tS, tU) {
      gl.viewport(0, 0, W, H);
      gl.disable(gl.BLEND);
      const salt = wasm.gl_backdrop(sys, bgPtr, cx, cy, zoom, bgx, bgy);
      gl.useProgram(progBg);
      gl.uniform1fv(progBg.loc.B, new Float32Array(wasm.memory.buffer, bgPtr, bgLen));
      gl.uniform1i(progBg.loc.u_skySalt, salt);
      gl.uniform1i(progBg.loc.u_vh, H);
      gl.drawArrays(gl.TRIANGLES, 0, 3);

      const np = wasm.gl_orbit_points(sys, orbPtr, orbCap, W, H, cx, cy, zoom);
      if (np > 0) {
        gl.enable(gl.BLEND); gl.blendFunc(gl.ONE, gl.ONE);
        gl.bindVertexArray(vao);
        gl.bindBuffer(gl.ARRAY_BUFFER, vbo);
        gl.bufferData(gl.ARRAY_BUFFER, new Float32Array(wasm.memory.buffer, orbPtr, np * 2), gl.DYNAMIC_DRAW);
        gl.useProgram(progOrbit);
        gl.uniform2f(progOrbit.loc.u_view, W, H);
        gl.uniform1f(progOrbit.loc.u_size, wasm.gl_orbit_width(sys));
        gl.drawArrays(gl.POINTS, 0, np);
        gl.bindVertexArray(null);
      }

      const nb = wasm.gl_bodies(sys, bodyPtr, W, H, cx, cy, zoom, tO, tS, tU);
      gl.enable(gl.BLEND);
      gl.blendFunc(gl.SRC_ALPHA, gl.ONE_MINUS_SRC_ALPHA);
      const recs = new Float32Array(wasm.memory.buffer, bodyPtr, nb * stride);
      for (let i = 0; i < nb; i++) {
        const r = i * stride, isStar = recs[r] === 0;
        const p = isStar ? progStar : progPlanet;
        const uni = recs.subarray(r + header, r + stride);
        gl.useProgram(p);
        gl.uniform4f(p.loc.u_rect, recs[r + 1], recs[r + 2], recs[r + 3], recs[r + 3]);
        gl.uniform2f(p.loc.u_view, W, H);
        gl.uniform1i(p.loc.u_size, recs[r + 4] | 0);
        gl.uniform1i(p.loc.u_vh, H);
        if (isStar) gl.uniform1fv(p.loc.S, uni.subarray(0, 32));
        else {
          gl.uniform1fv(p.loc.U, uni);
          gl.uniform1ui(p.loc.u_seed, 0);
          gl.uniform1ui(p.loc.u_feat, glFeat >>> 0);
          gl.uniform1i(p.loc.u_palette, 0);
        }
        gl.drawArrays(gl.TRIANGLE_STRIP, 0, 4);
      }
      gl.finish();
      return nb;
    }

    const rows = [];
    // Zoom levels chosen for what they switch: 0.6 fits the system (nebula and
    // far star layer at full strength), 3.0 crosses the far-layer fade, 9.0 is
    // zoomed onto a body with the clouds gone.
    for (const [name, seed, zoom] of [["fit", 7, 0.6], ["mid", 7, 3.0], ["zoom", 21, 9.0]]) {
      const sys = wasm.system_new(seed);
      wasm.system_set_frozen_clouds(sys, 0);   // the GPU runs the live shader
      const acc = { total: 0, diff: 0, big: 0, sum: 0, worst: 0 };
      let bodies = 0;
      for (const [cx, cy, bgx, bgy, t] of [[0, 0, 0, 0, 0.13], [40, -18, 220, -90, 0.61]]) {
        wasm.render_t(sys, bufPtr, W, H, cx, cy, zoom, bgx, bgy, t, t * 1.3, t * 0.7);
        const cpu = new Uint8Array(wasm.memory.buffer, bufPtr, n).slice();
        bodies = drawScene(sys, cx, cy, zoom, bgx, bgy, t, t * 1.3, t * 0.7);
        gl.readPixels(0, 0, W, H, gl.RGBA, gl.UNSIGNED_BYTE, raw);
        flipRows(raw, flipped, W, H);
        compare(cpu, flipped, acc);
      }
      rows.push({ label: `${name} z=${zoom}`, bodies, ...acc });
      wasm.system_free(sys);
    }
    return { renderer: gl.getParameter(gl.RENDERER), rows, dims: `${W}x${H}` };
  }, { bytes, size, helpers: GL_HELPERS });
}

// ---------------------------------------------------------------------------

const PLANET_NAMES = [
  "terran", "ocean", "archipelago", "desert", "swamp", "iron", "ice", "barren",
  "gas_giant", "ice_giant", "lava", "fungal", "savanna", "gaia", "tundra", "alpine",
  "obsidian", "chrome", "moon", "storm_giant", "ringed_giant", "molten_sea",
  "radioactive", "crystal", "toxic", "storm_shroud",
];

function report(title, out, nameOf) {
  if (out.fatal) {
    console.error(`verify-gl [${title}]: ${out.fatal}`);
    return false;
  }
  console.log(`\n=== ${title} ${out.dims ? `(${out.dims})` : `(${SIZE}x${SIZE})`} ===`);
  console.log(`${"case".padEnd(16)}${"differ".padStart(9)}${">1 level".padStart(10)}${"max".padStart(6)}${"mean|d|".padStart(9)}`);
  let worstBig = 0, worstMax = 0;
  for (const r of out.rows) {
    const pct = v => `${(100 * v / r.total).toFixed(2)}%`;
    worstBig = Math.max(worstBig, r.big / r.total);
    worstMax = Math.max(worstMax, r.worst);
    const mean = r.diff ? r.sum / r.diff : 0;
    console.log(
      `${nameOf(r).padEnd(16)}${pct(r.diff).padStart(9)}${pct(r.big).padStart(10)}` +
      `${String(r.worst).padStart(6)}${mean.toFixed(2).padStart(9)}`);
  }
  console.log(`worst rate past one quantization level: ${(100 * worstBig).toFixed(2)}%  (max delta ${worstMax}/255)`);
  if (worstBig > LIMIT) {
    console.error(`FAIL [${title}]: over ${(100 * LIMIT).toFixed(1)}% — the two shaders disagree structurally, not just at the quantizer.`);
    return false;
  }
  return true;
}

const { chromium } = await loadPlaywright();
// SwiftShader, because this container has no GPU. `--enable-unsafe-swiftshader`
// is what lets a headless Chromium hand out a WebGL2 context without one.
const browser = await chromium.launch({
  args: ["--enable-unsafe-swiftshader", "--use-gl=angle", "--use-angle=swiftshader"],
});
const page = await browser.newPage();
page.on("console", m => { if (VERBOSE || m.type() === "error") console.log(`  [page] ${m.text()}`); });
page.on("pageerror", e => console.log(`  [page] ${e.message}`));
await page.setContent("<!doctype html><meta charset=utf-8><body></body>");

let ok = true;
if (DEMO === "planet" || DEMO === "all") {
  const out = await checkPlanet(page, wasmOf("planet"), SIZE, ALL);
  if (!out.fatal) console.log(`renderer: ${out.renderer}`);
  ok = report("planet", out, r => PLANET_NAMES[+r.label] ?? `type ${r.label}`) && ok;
}
if (DEMO === "solar" || DEMO === "all") {
  const out = await checkSolar(page, wasmOf("solar"), DEMO === "all" ? 220 : SIZE);
  if (!out.fatal) console.log(`renderer: ${out.renderer}`);
  ok = report("solar", out, r => r.label) && ok;
}
await browser.close();

if (!ok) process.exit(1);
console.log("\nOK — every difference is confined to quantizer threshold flips.");
