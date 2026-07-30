// planet.glsl — the WebGL2 (GLSL ES 3.00) port of `render_frame`'s pixel loop.
//
// Concatenated after `noise_core::GL_PRELUDE` + `dither_core::GL_PRELUDE`, which
// carry the `#version` line, the precision qualifiers and the lattice kernels;
// this file must not repeat them.
//
// This is a SECOND implementation of the shader in lib.rs, which the "one planet
// renderer" rule otherwise forbids. It earns the exception by keeping the
// duplication down to the pixel loop and nothing else:
//
//   • Every constant it reads — the `PType` row, the colour ramp, the seed
//     offsets, the vortex centres, the moon orbits, the per-frame trig, the
//     `Lod` octave counts — is computed by `gl_uniforms()` in gl.rs and handed
//     over in `U[]`. The type table is not duplicated, only transported.
//   • The noise underneath is `hash3`, which is exact across the two languages.
//
// What is genuinely re-written here is the shading: ~200 lines of ramps, mixes
// and smoothsteps. `scripts/verify-gl.mjs` renders both paths in headless
// Chromium and diffs them, which is what keeps them honest.
//
// It renders the LIVE shader — no `F_BAKED_*`. The sphere maps exist to make the
// CPU afford the weather; a GPU evaluates the fBm directly, so the GL path shows
// the unfrozen picture and needs none of the bake's memory.
//
// TWO FRAMINGS, exactly as in lib.rs. `U_SPRITE` picks:
//   • hero  — the planet fills a `u_size` square over a starfield, opaque.
//   • tile  — `render_tile`'s cut-out: transparent off the disc, lit from any
//     direction, and placed on screen through the same mapping `scene_core`'s
//     `blit` uses, so a scene composites it with the rasterizer instead.
//
// Light basis is +x right, +y up, +z toward the viewer, as in lib.rs.

// ---------------------------------------------------------------------------
// The uniform block. Slot names MUST match `GL_U_*` in gl.rs — that pairing is
// the whole wire format.
// ---------------------------------------------------------------------------
#define U_BASE        0
#define U_FREQ        1
#define U_CONTRAST    2
#define U_RIDGED      3
#define U_CLOUDS      4
#define U_CAPS        5
#define U_BANDS       6
#define U_TURB        7
#define U_GLOW_E0     8
#define U_GLOW_E1     9
#define U_RINGS      10
#define U_RING_IN    11
#define U_RING_OUT   12
#define U_SPECULAR   13
#define U_SHININESS  14
#define U_SPEC_ALB   15
#define U_SPOT       16
#define U_LIGHTNING  17
#define U_AURORA     18
#define U_STORM      19
#define U_NSTOPS     20
#define U_HAS_ATMO   21
#define U_RAD        22
#define U_SINA       23
#define U_COSA       24
#define U_ANGLE      25
#define U_CS         26
#define U_CC         27
#define U_MORPH      28
#define U_SWIRL      29
#define U_NMOON      30
#define U_DITHER     31
#define U_ATMO       32   // vec3
#define U_LIGHT      35   // vec3
#define U_DARK       38   // vec3
#define U_ROCK       41   // vec3
#define U_GLOW_LO    44   // vec3
#define U_GLOW_HI    47   // vec3
#define U_RING_COL   50   // vec3
#define U_OFS        53   // vec3
#define U_L          56   // vec3, unit light direction
#define U_VORTEX     59   // 2 x (x, z)
#define U_MOON       63   // 2 x (mx, my, mr, depth, ms)
#define U_STOPS      73   // 7 x (threshold, r, g, b)
#define U_OCT       101   // 15 day counts, then the same 15 capped for night
#define U_PAL_LEN   131
#define U_PAL       132   // up to 5 x rgb
#define U_SPRITE    147   // 0 hero framing, 1 scene tile
#define U_TILE_X0   148   // `blit`'s destination rect origin, screen px
#define U_TILE_Y0   149
#define U_TILE_INV  150   // 1 / scale

// Octave slots, indexed off U_OCT (+15 for the night-capped copy).
#define O_SPOT_EDGE    0
#define O_SPOT_STREAK  1
#define O_AURORA       2
#define O_CRATER_M     3
#define O_TERR         4
#define O_BAND_W       5
#define O_BAND_WARP    6
#define O_BAND_FINE    7
#define O_EMIS_ROCK    8
#define O_EMIS_FLOW    9
#define O_CLOUDY       10
#define O_CLOUDY_WARP  11
#define O_DECK         12
#define O_DECK_WARP    13
#define O_SHIMMER      14

// `F_*` from lib.rs.
const uint F_CLOUD_SHADOW = 1u;
const uint F_ATMO         = 2u;
const uint F_RIM          = 4u;
const uint F_STARFIELD    = 8u;
const uint F_NIGHT_LOD    = 16u;

const float RING_SQUASH = 0.38;
const float SPEC_FLOOR  = 1.0 / 1024.0;

uniform float U[160];
uniform uint  u_seed;
uniform uint  u_feat;
uniform int   u_size;      // tile / frame edge in px
uniform int   u_vh;        // viewport height, for the y flip (== u_size for hero)
uniform int   u_palette;

out vec4 fragColor;

// True where this pixel is past the terminator and `F_NIGHT_LOD` applies, which
// selects the capped half of the octave table.
bool g_night = false;

vec3 uv3(int i) { return vec3(U[i], U[i + 1], U[i + 2]); }
int  oct(int slot) { return int(U[U_OCT + slot + (g_night ? 15 : 0)]); }

// The colour ramp is a hard step, exactly as in noise-core: the first stop whose
// threshold `h` falls under, else the last.
vec3 rampCol(float h) {
  int n = int(U[U_NSTOPS]);
  for (int i = 0; i < 7; i++) {
    if (i >= n) break;
    if (h < U[U_STOPS + i * 4]) return uv3(U_STOPS + i * 4 + 1);
  }
  return uv3(U_STOPS + (n - 1) * 4 + 1);
}

vec3 finalize(vec3 o, float bx) {
  int n = int(U[U_PAL_LEN]);
  if (u_palette > 0 && n > 0) {
    float lum = clamp(o.x * 0.3 + o.y * 0.59 + o.z * 0.11, 0.0, 1.0);
    float f = (lum + bx * 0.14) * (float(n) - 1.0);
    int i = int(min(max(f + 0.5, 0.0), float(n) - 1.0));
    return uv3(U_PAL + i * 3);
  }
  return quant(o, bx, 22.0, U[U_DITHER]);
}

// ---------------------------------------------------------------------------
// Weather
// ---------------------------------------------------------------------------

vec3 greatSpot(vec3 col, float sx, float sy, float sz, float angle, float intensity) {
  float spotLat = 0.28;
  float spotLon = 0.6 + sin(angle) * 0.18;
  float dlon = atan(sz, sx) - spotLon;
  if (dlon >  PI) dlon -= TAU;
  if (dlon < -PI) dlon += TAU;
  float dlat = sy - spotLat;
  float b = sqrt((dlon * 1.05) * (dlon * 1.05) + (dlat * 2.2) * (dlat * 2.2));
  // 0.82·base is the smallest the boundary below can make it, so this is an
  // exact rejection — and most of a banded disc takes it.
  if (b * 0.82 >= 1.0) return col;
  float edge = fbm(dlon * 3.0 + sy * 4.0, dlat * 3.0, sz * 2.0, oct(O_SPOT_EDGE));
  float d = b * (0.82 + 0.4 * edge);
  if (d >= 1.0) return col;
  float swirl = (1.0 - d) * 5.0 + angle * 1.2;
  float s = sin(swirl), c = cos(swirl);
  float lx = dlon * c - dlat * s;
  float ly = dlon * s + dlat * c;
  float streak = fbm(lx * 8.0, ly * 8.0, sy * 2.0, oct(O_SPOT_STREAK));
  float core = sstep(1.0, 0.15, d) * intensity;
  vec3 spotCol = lerp3(vec3(0.80, 0.36, 0.26), vec3(0.93, 0.66, 0.46), sstep(0.40, 0.82, streak));
  vec3 outc = lerp3(col, spotCol, core * 0.78);
  float eye = sstep(0.20, 0.06, d) * intensity;
  return lerp3(outc, vec3(0.28, 0.11, 0.10), eye * 0.7);
}

float auroraGlow(float sx, float sy, float sz, float angle) {
  float lat = abs(sy);
  float band = sstep(0.55, 0.70, lat) * (1.0 - sstep(0.82, 0.96, lat));
  if (band <= 0.0) return 0.0;
  float lon = atan(sz, sx);
  float curtain = fbm(lon * 2.5 + angle * 1.5, lat * 9.0, sy * 3.0 + angle, oct(O_AURORA));
  return band * sstep(0.48, 0.78, curtain);
}

// (intensity, colour) of the current lightning flash, or zero. Seeded per slot,
// so the rhythm is irregular and never repeats.
void lightningFlash(float sx, float sy, float angle, out float mag, out vec3 col) {
  mag = 0.0; col = vec3(0.0);
  const float SLOTS = 13.0;
  float t = angle * SLOTS / TAU;
  int slot = int(floor(t));
  float phase = t - floor(t);
  if (hash3(slot, 9, 5) > 0.5) return;
  float p = phase - hash3(slot, 8, 5) * 0.45;
  float env = sstep(0.0, 0.02, p) * (1.0 - sstep(0.05, 0.16, p));
  if (env <= 0.0) return;
  float intensity = 0.45 + hash3(slot, 7, 5) * 1.0;
  float hx = hash3(slot, 1, 5) * 2.0 - 1.0;
  float hy = (hash3(slot, 2, 5) * 2.0 - 1.0) * 0.7;
  float radius = 0.05 + hash3(slot, 3, 5) * 0.13;
  float d = sqrt((sx - hx) * (sx - hx) + (sy - hy) * (sy - hy));
  mag = env * intensity * sstep(radius, 0.0, d);
  float hue = hash3(slot, 4, 5);
  col = hue < 0.42 ? vec3(0.75, 0.83, 1.0)
      : hue < 0.66 ? vec3(0.82, 0.60, 1.0)
      : hue < 0.85 ? vec3(0.55, 0.95, 1.0)
                   : vec3(1.0, 0.90, 0.66);
}

// ---------------------------------------------------------------------------
// Surface
// ---------------------------------------------------------------------------

vec3 staticAlbedo(float sy, float px, float py, float pz) {
  float freq = U[U_FREQ];
  if (int(U[U_BASE]) == 1) {                              // Cratered
    float m = sstep(0.4, 0.6, fbm(px * 1.2, py * 1.2, pz * 1.2, oct(O_CRATER_M)));
    vec3 baseCol = lerp3(uv3(U_DARK), uv3(U_LIGHT), m);
    float w = worley(px * freq, py * freq, pz * freq);
    float bowl = sstep(0.0, 0.35, w);
    float rim = sstep(0.30, 0.42, w) * (1.0 - sstep(0.42, 0.60, w));
    return clamp(baseCol * (0.55 + 0.45 * bowl) + vec3(rim * 0.30), 0.0, 1.0);
  }
  float raw = fbm(px * freq, py * freq, pz * freq, oct(O_TERR));
  float n = U[U_RIDGED] > 0.5 ? 1.0 - abs(2.0 * raw - 1.0) : raw;
  vec3 col = rampCol(contrastf(n, U[U_CONTRAST]));
  float cap = sstep(0.72, 0.9, abs(sy)) * U[U_CAPS];
  return lerp3(col, vec3(0.92, 0.95, 1.0), cap);
}

void surface(float sx, float sy, float sz, float angle, out vec3 col, out float emis) {
  vec3 ofs = uv3(U_OFS);
  float px = sx + ofs.x, py = sy + ofs.y, pz = sz + ofs.z;
  int b = int(U[U_BASE]);
  emis = 0.0;

  if (b == 0 || b == 1) {                                 // Terrestrial / Cratered
    col = staticAlbedo(sy, px, py, pz);
  } else if (b == 2) {                                    // Banded
    // Zonal jets: adjacent latitude bands drift in opposite directions.
    float flow = angle * 0.16 * sin(sy * U[U_BANDS] * 0.5);
    float warp = fbmWarp((px + flow) * 1.3, py * 1.3, pz * 1.3, oct(O_BAND_WARP), oct(O_BAND_W), 0.8);
    float lat = sy + (warp - 0.5) * U[U_TURB];
    float fineN = fbm((px + flow * 1.4) * 4.0, py * 4.0, pz * 4.0, oct(O_BAND_FINE));
    float band = 0.5 + 0.5 * sin(lat * U[U_BANDS]);
    float fine = sstep(0.55, 0.8, fineN);
    col = lerp3(lerp3(uv3(U_DARK), uv3(U_LIGHT), band), uv3(U_LIGHT), fine * 0.35);
    if (U[U_SPOT] > 0.0) col = greatSpot(col, sx, sy, sz, angle, U[U_SPOT]);
  } else if (b == 3) {                                    // Emissive
    float freq = U[U_FREQ];
    float n = contrastf(fbm(px * freq, py * freq, pz * freq, oct(O_EMIS_ROCK)), 1.7);
    // A slow field advects across the surface, so the glow brightens and dims in
    // drifting patches instead of pulsing.
    float flow = fbm(px * 2.2 + angle * 0.7, py * 2.2, pz * 2.2 - angle * 0.5, oct(O_EMIS_FLOW));
    float glow = clamp(sstep(U[U_GLOW_E0], U[U_GLOW_E1], n) * (0.55 + 0.9 * flow), 0.0, 1.0);
    vec3 lo = uv3(U_GLOW_LO), hi = uv3(U_GLOW_HI);
    vec3 gcol = cycle3(lo, lerp3(lo, hi, 0.5), hi, n * 1.6 + angle * 0.12);
    col = lerp3(uv3(U_ROCK), gcol, glow);
    emis = glow;
  } else {                                                // Cloudy
    float flow = (0.5 + 0.3 * cos(sy * 3.0)) * sin(angle);
    float t = fbmWarp((px + flow) * 2.0, py * 2.0, pz * 2.0, oct(O_CLOUDY_WARP), oct(O_CLOUDY), 0.7);
    float band = 0.5 + 0.5 * sin(sy * U[U_BANDS] + (t - 0.5) * 6.0 * U[U_TURB]);
    col = lerp3(uv3(U_DARK), uv3(U_LIGHT), clamp(band * 0.6 + t * 0.4, 0.0, 1.0));
  }

  if (U[U_AURORA] > 0.0) {
    float a = auroraGlow(sx, sy, sz, angle) * U[U_AURORA];
    vec3 ac = cycle3(vec3(0.25, 0.95, 0.45), vec3(0.35, 0.85, 0.95), vec3(0.65, 0.40, 1.0),
                     sy * 1.4 + angle * 0.1);
    col = clamp(col + ac * a, 0.0, 1.0);
    emis = max(emis, a * 0.85);
  }
  if (U[U_LIGHTNING] > 0.0) {
    float mag; vec3 lc;
    lightningFlash(sx, sy, angle, mag, lc);
    float f = mag * U[U_LIGHTNING];
    col = clamp(col + lc * f, 0.0, 1.0);
    emis = max(emis, f);
  }
}

vec3 starBg(int ix, int iy) {
  float h = hash3(ix, iy, int(u_seed));
  if (h > 0.986) { float b = floor((150.0 + 105.0 * (h - 0.986) / 0.014)) / 255.0; return vec3(b); }
  return vec3(9.0, 8.0, 20.0) / 255.0;
}

// ---------------------------------------------------------------------------

void main() {
  bool sprite = U[U_SPRITE] > 0.5;
  int ix, iy;
  if (sprite) {
    // The tile pixel `scene_core::blit` would read for this destination pixel.
    // Same expression, so a body drawn here is blocky in exactly the places the
    // CPU compositor makes it blocky — `planet_pixel` and the detail cap keep
    // working with no second render target.
    int ddx = int(gl_FragCoord.x) - int(U[U_TILE_X0]);
    int ddy = (u_vh - 1 - int(gl_FragCoord.y)) - int(U[U_TILE_Y0]);
    ix = int((float(ddx) + 0.5) * U[U_TILE_INV]);
    iy = int((float(ddy) + 0.5) * U[U_TILE_INV]);
    if (ddx < 0 || ddy < 0 || ix >= u_size || iy >= u_size) discard;
  } else {
    ix = int(gl_FragCoord.x);
    iy = u_size - 1 - int(gl_FragCoord.y);
  }

  float rad = U[U_RAD];
  float cx = float(u_size) * 0.5, cy = cx;
  float nx = (float(ix) + 0.5 - cx) / rad;
  float ny = (cy - (float(iy) + 0.5)) / rad;
  float d2 = nx * nx + ny * ny;

  float angle = U[U_ANGLE];
  vec3 l = uv3(U_L);
  vec3 o = vec3(0.0);
  float a = 1.0;

  if (d2 <= 1.0) {
    float nz = sqrt(1.0 - d2);
    float sx = nx * U[U_COSA] + nz * U[U_SINA];
    float sy = ny;
    float sz = -nx * U[U_SINA] + nz * U[U_COSA];

    float diff = max(nx * l.x + ny * l.y + nz * l.z, 0.0);
    // Past the terminator `shade` bottoms out at the 0.10 ambient floor and 22
    // levels leave about three to say anything with, so the fine octaves and the
    // whole cloud deck are not computed there. Lightning fires anywhere on the
    // disc and emissive worlds are self-lit, so both opt out wholesale; aurora
    // opts out by latitude instead.
    bool nightOk = (u_feat & F_NIGHT_LOD) != 0u
                && U[U_LIGHTNING] == 0.0 && int(U[U_BASE]) != 3;
    g_night = nightOk && diff <= 0.0 && (U[U_AURORA] == 0.0 || abs(sy) < 0.52);

    vec3 col; float emis;
    surface(sx, sy, sz, angle, col, emis);

    if (U[U_CLOUDS] > 0.0 && !g_night) {
      vec3 ofs = uv3(U_OFS);
      // Clouds drift over the surface at 2x (parallax, and it loops).
      float cx3 = nx * U[U_CC] + nz * U[U_CS] + ofs.x;
      float cz3 = -nx * U[U_CS] + nz * U[U_CC] + ofs.z;
      if (U[U_STORM] > 0.0) {
        for (int k = 0; k < 2; k++) {
          float vx = U[U_VORTEX + k * 2], vz = U[U_VORTEX + k * 2 + 1];
          float dx = cx3 - vx, dz = cz3 - vz;
          float d2v = dx * dx + dz * dz;
          // exp(-2.2·d²) is under 1e-4 past here and only scales a rotation
          // angle, so the eddy does nothing. Most of a disc is this far out.
          if (d2v > 4.2) continue;
          float fall = exp(-d2v * 2.2);
          float ss = sin(fall * U[U_SWIRL]), sc = cos(fall * U[U_SWIRL]);
          cx3 = vx + dx * sc - dz * ss;
          cz3 = vz + dx * ss + dz * sc;
        }
      }
      float morph = U[U_MORPH];
      int n = oct(O_DECK);
      float my = ny * 2.8 + ofs.y + morph;
      // Wispy, domain-warped tops so the fronts break up; the shadow reads the
      // cheap plain density 0.45 toward the light.
      float cloud = fbmWarp(cx3 * 2.8, my, cz3 * 2.8 + morph, oct(O_DECK_WARP), n, 0.9);
      float shadow = sstep(0.55, 0.72,
        fbm((cx3 + l.x * 0.45) * 2.8, my, (cz3 + l.z * 0.45) * 2.8 + morph, n));
      if ((u_feat & F_CLOUD_SHADOW) != 0u) col *= 1.0 - 0.22 * shadow * U[U_CLOUDS];
      col = lerp3(col, vec3(1.0), sstep(0.52, 0.70, cloud) * U[U_CLOUDS]);
    }

    float shade = max(0.10 + 0.90 * diff, emis);
    o = col * shade;

    if (U[U_SPECULAR] > 0.0) {
      float hm = sqrt(l.x * l.x + l.y * l.y + (l.z + 1.0) * (l.z + 1.0));
      float ndh = max(nx * l.x / hm + ny * l.y / hm + nz * (l.z + 1.0) / hm, 0.0);
      // Darker surface reflects less: `col` is the un-shaded albedo, so a moon's
      // maria glare far less than its highlands.
      float alb = col.x * 0.3 + col.y * 0.59 + col.z * 0.11;
      float mat = 1.0 - U[U_SPEC_ALB] * (1.0 - alb);
      // `ndh^shininess` collapses fast, so bound the whole term before paying
      // for the shimmer fBm that only modulates it.
      float peak = pow(ndh, U[U_SHININESS]) * U[U_SPECULAR] * mat;
      if (peak > SPEC_FLOOR) {
        float shimmer = 0.82 + 0.18 * fbm(sx * 5.0 + angle * 2.5, sy * 5.0, sz * 5.0, oct(O_SHIMMER));
        o = clamp(o + vec3(peak * shimmer), 0.0, 1.0);
      }
    }
    if (U[U_HAS_ATMO] > 0.5 && (u_feat & F_ATMO) != 0u) {
      float t = 1.0 - nz;
      o = clamp(o + uv3(U_ATMO) * (t * t * t * 0.6), 0.0, 1.0);
    }
  } else if (sprite) {
    // Off the disc a tile is empty — the scene shows through.
    o = vec3(0.0);
    a = 0.0;
  } else {
    o = (u_feat & F_STARFIELD) != 0u ? starBg(ix, iy) : vec3(0.0);
  }

  if (U[U_RINGS] > 0.5) {
    float rr = sqrt(nx * nx + (ny / RING_SQUASH) * (ny / RING_SQUASH));
    float ri = U[U_RING_IN], ro = U[U_RING_OUT];
    if (rr >= ri && rr <= ro && (ny < 0.0 || d2 > 1.0)) {
      float rn = (rr - ri) / (ro - ri);
      float stripes = 0.5 + 0.5 * sin(rn * 36.0);
      float alpha = clamp(0.30 + 0.55 * stripes, 0.0, 1.0);
      if (rn > 0.46 && rn < 0.54) alpha *= 0.12;
      vec3 rc = uv3(U_RING_COL) * (0.55 + 0.45 * stripes);
      if (sprite && d2 > 1.0) {
        // The ring arc PAST the disc is the only translucent part of a tile.
        // Hand the scene the ring's own colour with `alpha` as its coverage and
        // let the blend do the mixing — lerping toward the empty tile here would
        // darken the ring by a second factor of `alpha` once composited.
        o = rc;
        a = alpha;
      } else {
        o = lerp3(o, rc, alpha);
      }
    }
  }

  // Crisp dark rim, BEFORE the moons so a front moon crossing the limb passes
  // over it instead of being clipped under it.
  if (d2 <= 1.0 && (u_feat & F_RIM) != 0u) {
    float edge = 1.0 - 1.3 / rad;
    if (d2 > edge * edge) o *= vec3(0.26, 0.26, 0.30);
  }

  int nmoon = int(U[U_NMOON]);
  for (int m = 0; m < 2; m++) {
    if (m >= nmoon) break;
    float mx = U[U_MOON + m * 5], my = U[U_MOON + m * 5 + 1], mr = U[U_MOON + m * 5 + 2];
    float depth = U[U_MOON + m * 5 + 3], ms = U[U_MOON + m * 5 + 4];
    float ld2 = (nx - mx) * (nx - mx) + (ny - my) * (ny - my);
    if (ld2 < mr * mr && (depth > 0.0 || d2 > 1.0)) {
      float lnx = (nx - mx) / mr, lny = (ny - my) / mr;
      float lnz = sqrt(max(1.0 - lnx * lnx - lny * lny, 0.0));
      float mdiff = max(lnx * l.x + lny * l.y + lnz * l.z, 0.0);
      float mrim = 0.4 + 0.6 * sstep(0.0, 0.26, lnz);
      float msh = (0.12 + 0.9 * mdiff) * mrim;
      float t = fbm(lnx * 3.0 + ms * 9.0, lny * 3.0, ms * 5.0, 2);
      vec3 base = lerp3(vec3(0.30, 0.29, 0.33), vec3(0.60, 0.59, 0.62), sstep(0.4, 0.6, t));
      o = base * msh;
      a = 1.0;
    }
  }

  if (sprite && a <= 0.0) discard;   // nothing to blend, and it saves the blend
  fragColor = vec4(toByte(finalize(o, bayer(ix, iy))), sprite ? floor(clamp(a, 0.0, 1.0) * 255.0) / 255.0 : 1.0);
}
