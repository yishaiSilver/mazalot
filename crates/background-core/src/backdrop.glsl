// backdrop.glsl — the GPU twin of `paint_backdrop` + `paint_stars`.
//
// Concatenated after `noise_core::GL_PRELUDE` + `dither_core::GL_PRELUDE`.
//
// This is the pass the worker pool could never reach: full-frame work that
// scales with the window, serial on the CPU, and repainted every frame whenever
// the camera follows a body (which invalidates `BackdropCache`'s key). On the
// GPU it is one fullscreen triangle and the whole cache disappears with it —
// there is nothing to memcpy, memmove, or key.
//
// Two structural differences from the CPU path, both because a fragment shader
// is per-pixel where the Rust is per-cell and per-star:
//
//   • **The cloud sprite is gone.** `BackdropCache` bakes an fBm field once per
//     8x8 cell and scrolls it; here each pixel finds its own cell directly.
//     `floor((scroll + i) / cell)` is the same world cell the Rust's
//     `org + (i + sub) / cell` split arrives at, so the clouds land in the same
//     places — the split only existed so a sprite could be slid.
//   • **Stars are gathered, not scattered.** `paint_stars` walks lit cells and
//     plots one pixel each; a fragment cannot do that, so it walks the 3x3 cells
//     that could possibly have placed a star on THIS pixel. Same hash, same
//     jitter, same `floor` — the pixel lights up under exactly the condition the
//     scatter would have lit it.
//
// The star hash is solar's convention (`hash3(cx, cy, salt + 17 + layer)`); a
// scene that mixes its seed in differently needs its own body here, which is
// what `paint_stars` takes a closure for on the CPU side.

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
#define B_NLAYERS    21
#define B_NTINTS     22
#define B_LAYER      24   // 4 x (ox, oy, spacing, thr, brightness, faint, amt, salt)
#define B_TINTS      56   // 8 x (cutoff, r, g, b)

uniform float B[96];
uniform int   u_skySalt;
uniform int   u_vh;      // viewport height, for the top-down row index

out vec4 fragColor;

/// Star colour: `noise_core::ramp` over the scene's tint stops.
vec3 starTint(float h) {
  int n = int(B[B_NTINTS]);
  for (int i = 0; i < 8; i++) {
    if (i >= n) break;
    if (h < B[B_TINTS + i * 4]) return vec3(B[B_TINTS + i * 4 + 1], B[B_TINTS + i * 4 + 2], B[B_TINTS + i * 4 + 3]);
  }
  return vec3(B[B_TINTS + (n - 1) * 4 + 1], B[B_TINTS + (n - 1) * 4 + 2], B[B_TINTS + (n - 1) * 4 + 3]);
}

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

/// Everything the parallax layers add at this pixel.
///
/// Gathered rather than scattered: for each layer, the cells that could have
/// placed their star here span one grid step in each direction, so a 3x3 sweep
/// about `floor((px + ox) / spacing)` is exhaustive.
vec3 starsAt(int ix, int iy) {
  vec3 add = vec3(0.0);
  int nl = int(B[B_NLAYERS]);
  for (int li = 0; li < 4; li++) {
    if (li >= nl) break;
    int b = B_LAYER + li * 8;
    float ox = B[b], oy = B[b + 1], spacing = B[b + 2], thr = B[b + 3];
    float brightness = B[b + 4], faint = B[b + 5], amt = B[b + 6];
    int salt = int(B[b + 7]);
    if (amt <= 0.02 || thr >= 0.9999) continue;    // faded out, or density ~0
    int c0x = int(floor((float(ix) + ox) / spacing));
    int c0y = int(floor((float(iy) + oy) / spacing));
    for (int dy = -1; dy <= 1; dy++) {
      for (int dx = -1; dx <= 1; dx++) {
        int cx = c0x + dx, cy = c0y + dy;
        float hh = hash3(cx, cy, u_skySalt + 17 + salt);
        if (hh <= thr) continue;
        float jx = fract(hh * 137.0);
        float jy = fract(hh * 71.3 + 0.37);
        int px = int(floor((float(cx) + jx) * spacing - ox));
        int py = int(floor((float(cy) + jy) * spacing - oy));
        if (px != ix || py != iy) continue;
        // How far this cell cleared the threshold sets its brightness.
        float t = (hh - thr) / (1.0 - thr);
        float s = brightness * (faint + (1.0 - faint) * t) * amt;
        add += s * starTint(fract(hh * 313.0));
      }
    }
  }
  return add;
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
  // The ground is written as bytes before the stars are added over it, so round
  // here too or a star sits on a background the CPU never had.
  col = toByte(clamp(col, 0.0, 1.0));
  fragColor = vec4(toByte(clamp(col + starsAt(ix, iy), 0.0, 1.0)), 1.0);
}
