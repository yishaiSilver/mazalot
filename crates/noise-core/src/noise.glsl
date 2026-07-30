#version 300 es
// noise.glsl — the GLSL ES 3.00 transliteration of lib.rs, and the prelude every
// shader in the workspace is built on.
//
// `#version` is first because some drivers reject a comment above it, even
// though the spec permits one. Everything downstream is concatenated after this
// file, so nothing else may carry a `#version` line.
//
// The integer half of this is EXACT, not approximate. `hash3` is wrapping u32
// multiplies, xors and shifts with no transcendental anywhere, and GLSL ES 3.00
// has all three with the same wrapping semantics — so the lattice under a GPU
// picture is bit-identical to the lattice under a CPU one. `value_noise`,
// `fbm`, `fbm_warp` and `worley` inherit that. What is NOT exact is the float
// shading built on top: a driver rounds `sin`/`exp`/`pow` its own way, and the
// output lands on 22 or 24 quantization levels, so a 1e-7 difference can become
// a whole level. See `scripts/verify-gl.mjs` for how that is measured.
//
// Kept in step with lib.rs by hand. The tests that guard the pairing live in
// each consuming crate; there is no way to compile Rust into a fragment shader.

precision highp float;
precision highp int;

const float PI  = 3.14159265358979323846;
const float TAU = 6.28318530717958647692;

// ---------------------------------------------------------------------------
// Hash
// ---------------------------------------------------------------------------

const uint KX = 0x8da6b343u;
const uint KY = 0xd8163841u;
const uint KZ = 0xcb1ab31fu;

uint avalanche(uint h) {
  h ^= h >> 16; h *= 0x7feb352du;
  h ^= h >> 15; h *= 0x846ca68bu;
  h ^= h >> 16;
  return h;
}

// `u32::MAX as f32` rounds to 2^32 in Rust and the literal rounds the same way
// here, so the scale is the same float. Divide rather than multiply by a
// reciprocal — that is what the Rust does.
float unitOf(uint h) { return float(avalanche(h)) / 4294967295.0; }

float hash3(int x, int y, int z) {
  return unitOf(uint(x) * KX ^ uint(y) * KY ^ uint(z) * KZ);
}

// ---------------------------------------------------------------------------
// Scalar helpers
// ---------------------------------------------------------------------------

float smoother(float t) { return t * t * t * (t * (t * 6.0 - 15.0) + 10.0); }

// `a + (b - a) * t`, NOT GLSL's `mix` (which is `a*(1-t) + b*t`) — the shading
// downstream lands on a couple of dozen quantization levels and the two round
// differently.
float lerpf(float a, float b, float t) { return a + (b - a) * t; }
vec3  lerp3(vec3 a, vec3 b, float t) {
  return vec3(lerpf(a.x, b.x, t), lerpf(a.y, b.y, t), lerpf(a.z, b.z, t));
}

// Reversed edges (e0 > e1) are used all over these shaders and are undefined for
// GLSL's `smoothstep`; noise-core's version handles them.
float sstep(float e0, float e1, float x) {
  float t = clamp((x - e0) / (e1 - e0), 0.0, 1.0);
  return t * t * (3.0 - 2.0 * t);
}

float contrastf(float h, float k) { return clamp((h - 0.5) * k + 0.5, 0.0, 1.0); }

/// Three-stop cool -> mid -> hot ramp.
vec3 ramp3(vec3 a, vec3 b, vec3 c, float t) {
  return t < 0.5 ? lerp3(a, b, t * 2.0) : lerp3(b, c, (t - 0.5) * 2.0);
}

/// Palette cycling: loop through a 3-stop gradient by phase, lo->mid->hi->lo.
vec3 cycle3(vec3 lo, vec3 mid, vec3 hi, float phase) {
  float p = (phase - floor(phase)) * 3.0;   // rem_euclid(1.0)
  if (p < 1.0) return lerp3(lo, mid, p);
  if (p < 2.0) return lerp3(mid, hi, p - 1.0);
  return lerp3(hi, lo, p - 2.0);
}

// ---------------------------------------------------------------------------
// Lattice kernels
// ---------------------------------------------------------------------------

float valueNoise(float x, float y, float z) {
  float xi = floor(x), yi = floor(y), zi = floor(z);
  float u = smoother(x - xi), v = smoother(y - yi), w = smoother(z - zi);
  // The x term is the only one that differs between the two corners of an edge,
  // so the y/z half of the mix is built once — the same factoring the Rust's
  // four-lane kernel uses, for the same reason.
  uint kx0 = uint(int(xi)) * KX, kx1 = uint(int(xi) + 1) * KX;
  uint ky0 = uint(int(yi)) * KY, ky1 = uint(int(yi) + 1) * KY;
  uint kz0 = uint(int(zi)) * KZ, kz1 = uint(int(zi) + 1) * KZ;
  uint a = ky0 ^ kz0, b = ky1 ^ kz0, c = ky0 ^ kz1, d = ky1 ^ kz1;
  float x00 = lerpf(unitOf(kx0 ^ a), unitOf(kx1 ^ a), u);
  float x10 = lerpf(unitOf(kx0 ^ b), unitOf(kx1 ^ b), u);
  float x01 = lerpf(unitOf(kx0 ^ c), unitOf(kx1 ^ c), u);
  float x11 = lerpf(unitOf(kx0 ^ d), unitOf(kx1 ^ d), u);
  return lerpf(lerpf(x00, x10, v), lerpf(x01, x11, v), w);
}

// Bounded trip count: nothing in the workspace asks for more than 6, and a
// constant ceiling keeps the loop unrollable on drivers that insist on it.
float fbm(float x, float y, float z, int octaves) {
  float sum = 0.0, amp = 0.5, norm = 0.0;
  for (int i = 0; i < 8; i++) {
    if (i >= octaves) break;
    sum += amp * valueNoise(x, y, z);
    norm += amp;
    amp *= 0.5;
    x *= 2.0; y *= 2.0; z *= 2.0;
  }
  return sum / norm;
}

float fbmWarp(float x, float y, float z, int warpOct, int mainOct, float w) {
  float qx = fbm(x, y, z, warpOct);
  float qy = fbm(x + 3.1, y + 1.7, z + 5.2, warpOct);
  float qz = fbm(x + 8.3, y + 2.8, z + 1.1, warpOct);
  return fbm(x + w * qx, y + w * qy, z + w * qz, mainOct);
}

// 3D Worley F1. Squared distance throughout, one sqrt at the end — `sqrt` is
// monotone, so this is the same float the per-cell version would give.
float worley(float x, float y, float z) {
  int fx = int(floor(x)), fy = int(floor(y)), fz = int(floor(z));
  float best = 81.0;
  for (int dz = -1; dz <= 1; dz++) {
    for (int dy = -1; dy <= 1; dy++) {
      for (int dx = -1; dx <= 1; dx++) {
        int cx = fx + dx, cy = fy + dy, cz = fz + dz;
        float px = float(cx) + hash3(cx, cy, cz) - x;
        float py = float(cy) + hash3(cx + 911, cy + 733, cz + 512) - y;
        float pz = float(cz) + hash3(cx + 271, cy + 619, cz + 188) - z;
        best = min(best, px * px + py * py + pz * pz);
      }
    }
  }
  return sqrt(best);
}

/// Bounded, decorrelated noise offsets from a seed — see `seed_offsets` in
/// lib.rs. `span` is 256 for planet/star, 220 for the scene crates.
vec3 seedOffsets(int seed, float span) {
  return vec3(hash3(seed, 1, 7), hash3(seed, 2, 7), hash3(seed, 3, 7)) * span + 4.0;
}
