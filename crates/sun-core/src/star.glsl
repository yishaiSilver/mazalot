// star.glsl — the GPU twin of `render_star_tile_into`.
//
// Concatenated after `noise_core::GL_PRELUDE` + `dither_core::GL_PRELUDE`.
//
// One deliberate difference from lib.rs: **the `Shade` tables are gone.** They
// exist because the halo is ~65% of a star tile's pixels and its three fields —
// streamer brightness, corona falloff, limb darkening — each vary along a single
// axis, so a CPU would rather sample them once per bake than once per pixel. A
// fragment shader has no such problem, so this evaluates all three directly.
//
// That makes the GPU path the *more* accurate of the two, and by a known margin:
// `tabulated_shading_matches_direct_evaluation` pins the CPU tables against
// exactly these expressions and finds them exact to the byte from radius 24 up.
// It also means `diamond_angle` has no purpose here — it was only ever a table
// index — so the streamers read their direction straight off `(nx, ny) / r`.
//
// The tile is placed through the same mapping `scene_core::blit` uses, so the
// star is blocky in the same places `sun_pixel` and the detail cap make it
// blocky on the CPU.

// ---------------------------------------------------------------------------
// Uniform block — slot names MUST match `GL_S_*` in gl.rs.
// ---------------------------------------------------------------------------
#define S_COOL        0   // vec3
#define S_MID         3   // vec3
#define S_HOT         6   // vec3
#define S_CORONA      9   // vec3
#define S_GRAN       12   // granulation cell frequency
#define S_OFS        13   // vec3, seed offsets (span 220)
#define S_T          16   // boil clock
#define S_RAD        17   // disc radius, tile px
#define S_REACH      18   // corona radius past the disc, in disc radii
#define S_WARP_OCT   19
#define S_BLOTCH_OCT 20
#define S_CORONA_OCT 21
#define S_TILE_X0    22   // `blit`'s destination rect origin, screen px
#define S_TILE_Y0    23
#define S_TILE_INV   24   // 1 / scale

uniform float S[32];
uniform int   u_size;    // tile edge in px
uniform int   u_vh;      // viewport height, for the top-down row index

out vec4 fragColor;

/// Per-pixel star surface shade — `star_surface` in lib.rs, with the limb table
/// evaluated rather than sampled.
vec3 starSurface(float sx, float sy, float sz, float mu) {
  float f = S[S_GRAN], t = S[S_T];
  vec3 ofs = vec3(S[S_OFS], S[S_OFS + 1], S[S_OFS + 2]);
  float px = sx + ofs.x, py = sy + ofs.y, pz = sz + ofs.z;
  // Boil the cell field slowly over time; sample a warped worley for lanes.
  float warp = 0.5 * fbm(px * 1.6 + t * 0.4, py * 1.6, pz * 1.6 - t * 0.3, int(S[S_WARP_OCT])) - 0.25;
  float w = worley(px * f + warp, py * f + warp, pz * f);
  float blotch = fbm(px * 0.9, py * 0.9, pz * 0.9 + t * 0.2, int(S[S_BLOTCH_OCT]));
  float coolRegion = sstep(0.46, 0.30, blotch);
  float lane = sstep(0.55, 0.82, w);
  float dark = clamp(coolRegion * 0.85 + lane * 0.4, 0.0, 1.0);
  float heat = clamp(1.0 - 0.9 * dark, 0.0, 1.0);
  vec3 cool = vec3(S[S_COOL], S[S_COOL + 1], S[S_COOL + 2]);
  vec3 col = ramp3(cool, vec3(S[S_MID], S[S_MID + 1], S[S_MID + 2]),
                   vec3(S[S_HOT], S[S_HOT + 1], S[S_HOT + 2]), heat);
  // Gentle limb darkening: dimmer + cooler at the edge for a spherical read.
  float limb = 0.66 + 0.34 * pow(mu, 0.45);
  col = lerp3(lerp3(col, cool, 0.20 * (1.0 - mu)), col, sqrt(mu));
  return col * limb;
}

void main() {
  // The tile pixel `scene_core::blit` would read for this destination pixel.
  int ddx = int(gl_FragCoord.x) - int(S[S_TILE_X0]);
  int ddy = (u_vh - 1 - int(gl_FragCoord.y)) - int(S[S_TILE_Y0]);
  int ix = int((float(ddx) + 0.5) * S[S_TILE_INV]);
  int iy = int((float(ddy) + 0.5) * S[S_TILE_INV]);
  if (ddx < 0 || ddy < 0 || ix >= u_size || iy >= u_size) discard;

  float rad = S[S_RAD], reach = S[S_REACH];
  float c = float(u_size) * 0.5;
  float nx = (float(ix) + 0.5 - c) / rad;
  float ny = (c - (float(iy) + 0.5)) / rad;
  float d2 = nx * nx + ny * ny;
  float r = sqrt(d2);

  vec3 col = vec3(0.0);
  float a = 0.0;
  if (d2 <= 1.0) {
    float nz = sqrt(1.0 - d2);
    col = starSurface(nx, ny, nz, nz);
    a = 1.0;
  }
  // Corona halo: a soft, shimmering falloff past the limb.
  float edge = r - 1.0;
  if (edge > 0.0 && edge < reach) {
    // Out here `r > 1`, so the unit direction the streamers want is just
    // `(nx, ny) / r` — no atan2/cos/sin round-trip, and no table.
    float invR = 1.0 / r;
    float flare = 0.6 + 0.5 * fbm(nx * invR * 5.0, ny * invR * 5.0, S[S_T] * 0.6, int(S[S_CORONA_OCT]));
    float fall = pow(sstep(reach, 0.0, edge), 1.6);
    float glow = clamp(fall * flare, 0.0, 1.0);
    vec3 cc = vec3(S[S_CORONA], S[S_CORONA + 1], S[S_CORONA + 2]) * glow;
    col = clamp(col * a + cc, 0.0, 1.0);
    a = clamp(max(a, glow), 0.0, 1.0);
  }
  if (a <= 0.0) discard;   // past the corona a tile is empty

  fragColor = vec4(toByte(quant(col, bayer(ix, iy), 24.0, 0.7)), floor(clamp(a, 0.0, 1.0) * 255.0) / 255.0);
}
