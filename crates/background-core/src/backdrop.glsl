// backdrop.glsl — the GPU twin of `paint_backdrop`.
//
// Concatenated after `noise_core::GL_PRELUDE` + `dither_core::GL_PRELUDE`.
//
// This is the pass the worker pool could never reach: full-frame work that
// scales with the window, serial on the CPU, and repainted every frame whenever
// the camera follows a body (which invalidates `BackdropCache`'s key). On the
// GPU it is one fullscreen triangle and the whole cache disappears with it —
// there is nothing to memcpy, memmove, or key.
//
// **Ground and nebula only** — the stars are point sprites, not fragments. A
// gather (each pixel asking which of nine cells per layer could have lit it) is
// what a fragment shader would have to do, and it measured as HALF the frame:
// 27 hashes per pixel against the scatter's roughly one per 50. `visit_stars`
// feeds `gl_star_points` instead, and the GPU draws what the CPU walk found.
//
// One structural difference from `paint_backdrop` remains. **The cloud sprite is
// gone**: `BackdropCache` bakes an fBm field once per 8x8 cell and scrolls it,
// where each pixel here finds its own cell directly.
// `floor((scroll + i) / cell)` is the same world cell the Rust's
// `org + (i + sub) / cell` split arrives at, so the clouds land in the same
// places — the split only existed so a sprite could be slid. It does mean 64x
// the noise evaluations for the same per-cell value, which is the next thing to
// port if the backdrop is ever the bottleneck.

// ---------------------------------------------------------------------------
// Uniform block — slot names MUST match `GL_B_*` in gl.rs.
// ---------------------------------------------------------------------------
#define B_BASE        0   // vec3, the colour of empty space
#define B_DITHER      3   // ground dither amplitude (0 under a nebula)
#define B_SHOW        4   // 1 when the nebula is drawn at all
#define B_CELL        5   // px per fBm sample
#define B_SCROLL_X    6   // cloud drift in px, already snapped to `quant`
#define B_SCROLL_Y    7
#define B_PHASE_X     8   // the dither's travel with the clouds, mod 8
#define B_PHASE_Y     9
#define B_STRENGTH   10
#define B_NEB_DITHER 11
#define B_NEB_AMT    12   // zoom fade
#define B_ZA         13   // the two seeded noise planes
#define B_ZB         14
#define B_TINT_A     15   // vec3
#define B_TINT_B     18   // vec3

uniform float B[24];
uniform int   u_vh;      // viewport height, for the top-down row index

out vec4 fragColor;

/// The nebula's contribution at screen pixel `(ix, iy)`, already scaled by the
/// zoom fade. Zero when the clouds are off or this cell came out empty.
vec3 nebulaAt(int ix, int iy) {
  if (B[B_SHOW] < 0.5) return vec3(0.0);
  float cell = B[B_CELL];
  // The absolute world cell this pixel sits in. The Rust splits the same value
  // into a whole-cell sprite origin plus a sub-cell read offset; nothing here
  // needs the split.
  float wx = floor((B[B_SCROLL_X] + float(ix)) / cell);
  float wy = floor((B[B_SCROLL_Y] + float(iy)) / cell);
  const float F = 1.0 / 240.0;
  float gx = wx * cell * F;
  float gy = wy * cell * F;
  float dens = sstep(0.50, 0.74, fbm(gx, gy, B[B_ZA], 3));   // patchy -> not crowded
  if (dens <= 0.0) return vec3(0.0);
  float n2 = fbm(gx * 1.8 + 40.0, gy * 1.8 + 7.0, B[B_ZB], 2);
  vec3 col = lerp3(vec3(B[B_TINT_A], B[B_TINT_A + 1], B[B_TINT_A + 2]),
                   vec3(B[B_TINT_B], B[B_TINT_B + 1], B[B_TINT_B + 2]),
                   clamp((n2 - 0.35) * 2.2, 0.0, 1.0));
  return col * (dens * B[B_STRENGTH] * B[B_NEB_AMT]);
}

void main() {
  int ix = int(gl_FragCoord.x);
  int iy = u_vh - 1 - int(gl_FragCoord.y);

  vec3 base = vec3(B[B_BASE], B[B_BASE + 1], B[B_BASE + 2]);
  float g = B[B_DITHER] > 0.0 ? bayer(ix, iy) * B[B_DITHER] : 0.0;
  vec3 col = base + vec3(g);

  if (B[B_SHOW] >= 0.5) {
    // The nebula's own dither is anchored to the CLOUDS, not the screen, so it
    // travels with them — which on the CPU is what let the whole layer scroll.
    float d = bayer(ix + int(B[B_PHASE_X]), iy + int(B[B_PHASE_Y])) * B[B_NEB_DITHER];
    col += max(nebulaAt(ix, iy) + vec3(d), 0.0);
  }
  // Rounded to bytes here because the CPU writes the ground out as bytes before
  // `paint_stars` adds over it, and the star points blend onto this.
  fragColor = vec4(toByte(clamp(col, 0.0, 1.0)), 1.0);
}
