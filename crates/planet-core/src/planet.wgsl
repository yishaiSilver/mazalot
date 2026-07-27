// planet.wgsl — the hero framing (planet-core's `render_rgba_styled`) as a
// fragment shader, one invocation per pixel.
//
// This is a deliberate second implementation of the shader in `lib.rs`, for the
// browser demo only: the sphere is embarrassingly parallel, and moving it to the
// GPU is what lets the demo run at 512² instead of 64². Read `lib.rs` first —
// the comments there explain *why* each constant is what it is, and are not
// repeated here. What IS repeated here is anything that would be a silent trap
// on the GPU (WGSL builtins whose edge-case behaviour differs from Rust's).
//
// Non-goals, so nobody chases them: this does not produce byte-identical output
// to the CPU path (see gpu.rs), and it implements only the hero framing — the
// `render_tile` sprite framing that `solar` blits stays CPU-side, where the
// scene compositor lives.
//
// Data comes in as two untyped f32 storage buffers:
//   `types` — the whole `TYPES` table, `STRIDE` floats per row, written by
//             `planet_core::gpu::type_table`. The `F_*` offsets below MUST stay
//             in lockstep with the ones in gpu.rs; a test pins them.
//   `fr`    — per-frame state (see the `FR_*` offsets).

const PI: f32 = 3.14159265358979;
const TAU: f32 = 6.28318530717959;

// -- `types` row layout. Mirrors the F_* constants in gpu.rs. -----------------
const MAX_STOPS: u32 = 8u;
const STRIDE: u32 = 76u;
const F_BASE: u32 = 0u;
const F_FREQ: u32 = 1u;
const F_CONTRAST: u32 = 2u;
const F_RIDGED: u32 = 3u;
const F_CLOUDS: u32 = 4u;
const F_CAPS: u32 = 5u;
const F_BANDS: u32 = 6u;
const F_TURB: u32 = 7u;
const F_GLOW_E0: u32 = 8u;
const F_GLOW_E1: u32 = 9u;
const F_RINGS: u32 = 10u;
const F_RING_INNER: u32 = 11u;
const F_RING_OUTER: u32 = 12u;
const F_RADIUS_SCALE: u32 = 13u;
const F_SPECULAR: u32 = 14u;
const F_SHININESS: u32 = 15u;
const F_SPEC_ALBEDO: u32 = 16u;
const F_SPOT: u32 = 17u;
const F_LIGHTNING: u32 = 18u;
const F_AURORA: u32 = 19u;
const F_STORM_CELLS: u32 = 20u;
const F_NSTOPS: u32 = 21u;
const F_ATMO: u32 = 22u;
const F_LIGHT: u32 = 25u;
const F_DARK: u32 = 28u;
const F_ROCK: u32 = 31u;
const F_GLOW_LO: u32 = 34u;
const F_GLOW_HI: u32 = 37u;
const F_RING_COL: u32 = 40u;
const F_STOPS: u32 = 43u;

// -- Base discriminants, numbered by gpu.rs::base_code. -----------------------
const B_TERRESTRIAL: u32 = 0u;
const B_CRATERED: u32 = 1u;
const B_BANDED: u32 = 2u;
const B_EMISSIVE: u32 = 3u;
const B_CLOUDY: u32 = 4u;

// -- `fr` per-frame layout. Mirrors FRAME_* in the demo's JS. -----------------
const FR_SIZE: u32 = 0u;
const FR_ANGLE: u32 = 1u;
const FR_SEED: u32 = 2u; // u32 bit pattern, not a number — bitcast it
const FR_TYPE: u32 = 3u;
const FR_PALETTE: u32 = 4u;
const FR_DITHER: u32 = 5u;
const FR_MOONS: u32 = 6u;
const FR_PARAMS: u32 = 7u; // 13 slider overrides, order per planet_core::param
const FR_STRIDE: u32 = 20u;

@group(0) @binding(0) var<storage, read> types: array<f32>;
@group(0) @binding(1) var<storage, read> fr: array<f32>;

// ---------------------------------------------------------------------------
// The active planet type, loaded once per pixel and then read as a global so
// the shading functions don't each copy 76 floats around.
// ---------------------------------------------------------------------------

struct PT {
    base: u32,
    freq: f32,
    contrast: f32,
    ridged: bool,
    clouds: f32,
    caps: f32,
    bands: f32,
    turb: f32,
    glow_e0: f32,
    glow_e1: f32,
    rings: bool,
    ring_inner: f32,
    ring_outer: f32,
    radius_scale: f32,
    specular: f32,
    shininess: f32,
    spec_albedo: f32,
    spot: f32,
    lightning: f32,
    aurora: f32,
    storm_cells: f32,
    nstops: u32,
    atmo: vec3<f32>,
    light: vec3<f32>,
    dark: vec3<f32>,
    rock: vec3<f32>,
    glow_lo: vec3<f32>,
    glow_hi: vec3<f32>,
    ring_col: vec3<f32>,
    stops: array<vec4<f32>, MAX_STOPS>, // (threshold, r, g, b)
}

var<private> ct: PT;

fn row_rgb(base_i: u32, at: u32) -> vec3<f32> {
    return vec3<f32>(types[base_i + at], types[base_i + at + 1u], types[base_i + at + 2u]);
}

/// Load row `type_idx` and apply the 13 slider overrides, exactly as
/// `planet_core::render_rgba_styled` does before it starts shading.
fn load_type(type_idx: u32) {
    let b = type_idx * STRIDE;
    ct.base = u32(types[b + F_BASE]);
    ct.ridged = types[b + F_RIDGED] != 0.0;
    ct.rings = types[b + F_RINGS] != 0.0;
    ct.ring_inner = types[b + F_RING_INNER];
    ct.ring_outer = types[b + F_RING_OUTER];
    ct.radius_scale = types[b + F_RADIUS_SCALE];
    ct.glow_e0 = types[b + F_GLOW_E0];
    ct.glow_e1 = types[b + F_GLOW_E1];
    ct.atmo = row_rgb(b, F_ATMO);
    ct.light = row_rgb(b, F_LIGHT);
    ct.dark = row_rgb(b, F_DARK);
    ct.rock = row_rgb(b, F_ROCK);
    ct.glow_lo = row_rgb(b, F_GLOW_LO);
    ct.glow_hi = row_rgb(b, F_GLOW_HI);
    ct.ring_col = row_rgb(b, F_RING_COL);
    ct.nstops = u32(types[b + F_NSTOPS]);
    for (var i = 0u; i < MAX_STOPS; i = i + 1u) {
        let at = b + F_STOPS + i * 4u;
        ct.stops[i] = vec4<f32>(types[at], types[at + 1u], types[at + 2u], types[at + 3u]);
    }
    // Slider-driven fields come from `fr`, not the table — same 13, same order
    // as planet_core::param.
    ct.contrast = fr[FR_PARAMS + 0u];
    ct.freq = fr[FR_PARAMS + 1u];
    ct.specular = fr[FR_PARAMS + 2u];
    ct.shininess = fr[FR_PARAMS + 3u];
    ct.clouds = fr[FR_PARAMS + 4u];
    ct.caps = fr[FR_PARAMS + 5u];
    ct.spot = fr[FR_PARAMS + 6u];
    ct.lightning = fr[FR_PARAMS + 7u];
    ct.aurora = fr[FR_PARAMS + 8u];
    ct.storm_cells = fr[FR_PARAMS + 9u];
    ct.bands = fr[FR_PARAMS + 10u];
    ct.turb = fr[FR_PARAMS + 11u];
    ct.spec_albedo = fr[FR_PARAMS + 12u];
}

// ---------------------------------------------------------------------------
// noise-core / dither-core primitives
// ---------------------------------------------------------------------------

fn hash3(x: i32, y: i32, z: i32) -> f32 {
    // Murmur3-style bit mixer. WGSL u32 arithmetic wraps, matching Rust's
    // wrapping_mul; the i32 inputs are reinterpreted, matching `x as u32`.
    var h: u32 = (bitcast<u32>(x) * 0x8da6b343u)
        ^ (bitcast<u32>(y) * 0xd8163841u)
        ^ (bitcast<u32>(z) * 0xcb1ab31fu);
    h = h ^ (h >> 16u);
    h = h * 0x7feb352du;
    h = h ^ (h >> 15u);
    h = h * 0x846ca68bu;
    h = h ^ (h >> 16u);
    return f32(h) / 4294967295.0;
}

fn smoother(t: f32) -> f32 {
    return t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
}

/// `noise_core::lerp`. NOT WGSL's `mix`: mix computes `a*(1-t) + b*t`, which
/// rounds differently from `a + (b-a)*t` and would shift pixels across dither
/// thresholds relative to the CPU path for no reason.
fn lerpf(a: f32, b: f32, t: f32) -> f32 {
    return a + (b - a) * t;
}

fn lerp3(a: vec3<f32>, b: vec3<f32>, t: f32) -> vec3<f32> {
    return a + (b - a) * t;
}

fn clamp01(x: f32) -> f32 {
    return clamp(x, 0.0, 1.0);
}

fn clamp01v(v: vec3<f32>) -> vec3<f32> {
    return clamp(v, vec3<f32>(0.0), vec3<f32>(1.0));
}

/// `noise_core::smoothstep`. NOT WGSL's `smoothstep`, whose result is undefined
/// when `e0 >= e1` — and the shader calls it that way on purpose all over the
/// place (`sstep(1.0, 0.15, d)`, `sstep(radius, 0.0, d)`, …) to get a falling
/// ramp. Rust's version just divides by a negative span and works.
fn sstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = clamp01((x - e0) / (e1 - e0));
    return t * t * (3.0 - 2.0 * t);
}

fn contrastf(h: f32, k: f32) -> f32 {
    return clamp01((h - 0.5) * k + 0.5);
}

fn value_noise(x: f32, y: f32, z: f32) -> f32 {
    let fx = floor(x);
    let fy = floor(y);
    let fz = floor(z);
    let xi = i32(fx);
    let yi = i32(fy);
    let zi = i32(fz);
    let u = smoother(x - fx);
    let v = smoother(y - fy);
    let w = smoother(z - fz);
    let x00 = lerpf(hash3(xi, yi, zi), hash3(xi + 1, yi, zi), u);
    let x10 = lerpf(hash3(xi, yi + 1, zi), hash3(xi + 1, yi + 1, zi), u);
    let x01 = lerpf(hash3(xi, yi, zi + 1), hash3(xi + 1, yi, zi + 1), u);
    let x11 = lerpf(hash3(xi, yi + 1, zi + 1), hash3(xi + 1, yi + 1, zi + 1), u);
    return lerpf(lerpf(x00, x10, v), lerpf(x01, x11, v), w);
}

fn fbm(px: f32, py: f32, pz: f32, octaves: u32) -> f32 {
    var x = px;
    var y = py;
    var z = pz;
    var sum = 0.0;
    var amp = 0.5;
    var norm = 0.0;
    for (var i = 0u; i < octaves; i = i + 1u) {
        sum = sum + amp * value_noise(x, y, z);
        norm = norm + amp;
        amp = amp * 0.5;
        x = x * 2.0;
        y = y * 2.0;
        z = z * 2.0;
    }
    return sum / norm;
}

fn fbm_warp(x: f32, y: f32, z: f32, octaves: u32, w: f32) -> f32 {
    let qx = fbm(x, y, z, octaves);
    let qy = fbm(x + 3.1, y + 1.7, z + 5.2, octaves);
    let qz = fbm(x + 8.3, y + 2.8, z + 1.1, octaves);
    return fbm(x + w * qx, y + w * qy, z + w * qz, octaves);
}

fn worley(x: f32, y: f32, z: f32) -> f32 {
    let fx = i32(floor(x));
    let fy = i32(floor(y));
    let fz = i32(floor(z));
    var f1 = 9.0;
    for (var dz = -1; dz <= 1; dz = dz + 1) {
        for (var dy = -1; dy <= 1; dy = dy + 1) {
            for (var dx = -1; dx <= 1; dx = dx + 1) {
                let cx = fx + dx;
                let cy = fy + dy;
                let cz = fz + dz;
                let p = vec3<f32>(
                    f32(cx) + hash3(cx, cy, cz),
                    f32(cy) + hash3(cx + 911, cy + 733, cz + 512),
                    f32(cz) + hash3(cx + 271, cy + 619, cz + 188),
                );
                f1 = min(f1, length(p - vec3<f32>(x, y, z)));
            }
        }
    }
    return f1;
}

fn seed_offsets(seed: u32) -> vec3<f32> {
    let s = bitcast<i32>(seed);
    return vec3<f32>(
        hash3(s, 1, 7) * 256.0 + 4.0,
        hash3(s, 2, 7) * 256.0 + 4.0,
        hash3(s, 3, 7) * 256.0 + 4.0,
    );
}

/// `noise_core::ramp` — first stop whose threshold the value falls under, and
/// the last stop past the end.
fn ramp_col(h: f32) -> vec3<f32> {
    if (ct.nstops == 0u) {
        return vec3<f32>(0.0);
    }
    for (var i = 0u; i < ct.nstops; i = i + 1u) {
        if (h < ct.stops[i].x) {
            return ct.stops[i].yzw;
        }
    }
    return ct.stops[ct.nstops - 1u].yzw;
}

fn cycle3(lo: vec3<f32>, mid: vec3<f32>, hi: vec3<f32>, phase: f32) -> vec3<f32> {
    // Rust uses rem_euclid(1.0), which for f32 is exactly fract().
    let p = fract(phase) * 3.0;
    if (p < 1.0) {
        return lerp3(lo, mid, p);
    } else if (p < 2.0) {
        return lerp3(mid, hi, p - 1.0);
    }
    return lerp3(hi, lo, p - 2.0);
}

// 8x8 ordered (Bayer) matrix, values 0..63 — dither_core::BAYER.
const BAYER = array<u32, 64>(
    0u, 32u, 8u, 40u, 2u, 34u, 10u, 42u, 48u, 16u, 56u, 24u, 50u, 18u, 58u, 26u,
    12u, 44u, 4u, 36u, 14u, 46u, 6u, 38u, 60u, 28u, 52u, 20u, 62u, 30u, 54u, 22u,
    3u, 35u, 11u, 43u, 1u, 33u, 9u, 41u, 51u, 19u, 59u, 27u, 49u, 17u, 57u, 25u,
    15u, 47u, 7u, 39u, 13u, 45u, 5u, 37u, 63u, 31u, 55u, 23u, 61u, 29u, 53u, 21u,
);

fn bayer(x: u32, y: u32) -> f32 {
    return (f32(BAYER[(y % 8u) * 8u + (x % 8u)]) + 0.5) / 64.0 - 0.5;
}

/// `dither_core::quant`. `floor(v + 0.5)` rather than WGSL's `round()`, which
/// breaks ties to even where Rust's `f32::round` breaks them away from zero.
fn quant(o: vec3<f32>, bx: f32, levels: f32, dither: f32) -> vec3<f32> {
    let d = bx * dither / levels;
    return clamp01v(floor((o + vec3<f32>(d)) * levels + vec3<f32>(0.5)) / levels);
}

// ---------------------------------------------------------------------------
// Weather
// ---------------------------------------------------------------------------

fn great_spot(col: vec3<f32>, sx: f32, sy: f32, sz: f32, angle: f32, intensity: f32) -> vec3<f32> {
    let spot_lat = 0.28;
    let spot_lon = 0.6 + sin(angle) * 0.18;
    let lon = atan2(sz, sx);
    // Rust wraps with a pair of while loops; over these ranges that is exactly
    // one symmetric fold into (-PI, PI].
    var dlon = lon - spot_lon;
    dlon = dlon - TAU * round(dlon / TAU);
    let dlat = sy - spot_lat;
    let edge = fbm(dlon * 3.0 + sy * 4.0, dlat * 3.0, sz * 2.0, 2u);
    let d = sqrt(dlon * 1.05 * dlon * 1.05 + dlat * 2.2 * dlat * 2.2) * (0.82 + 0.4 * edge);
    if (d >= 1.0) {
        return col;
    }
    let swirl = (1.0 - d) * 5.0 + angle * 1.2;
    let s = sin(swirl);
    let c = cos(swirl);
    let lx = dlon * c - dlat * s;
    let ly = dlon * s + dlat * c;
    let streak = fbm(lx * 8.0, ly * 8.0, sy * 2.0, 4u);
    let core = sstep(1.0, 0.15, d) * intensity;
    let spot_col = lerp3(vec3<f32>(0.80, 0.36, 0.26), vec3<f32>(0.93, 0.66, 0.46), sstep(0.40, 0.82, streak));
    var out = lerp3(col, spot_col, core * 0.78);
    let eye = sstep(0.20, 0.06, d) * intensity;
    return lerp3(out, vec3<f32>(0.28, 0.11, 0.10), eye * 0.7);
}

fn aurora_glow(sx: f32, sy: f32, sz: f32, angle: f32) -> f32 {
    let lat = abs(sy);
    let band = sstep(0.55, 0.70, lat) * (1.0 - sstep(0.82, 0.96, lat));
    if (band <= 0.0) {
        return 0.0;
    }
    let lon = atan2(sz, sx);
    let curtain = fbm(lon * 2.5 + angle * 1.5, lat * 9.0, sy * 3.0 + angle, 3u);
    return band * sstep(0.48, 0.78, curtain);
}

/// (intensity, colour) of the storm flash at this point. `vec4` because WGSL has
/// no tuples: xyz is the colour, w the magnitude.
fn lightning_flash(sx: f32, sy: f32, angle: f32) -> vec4<f32> {
    const SLOTS: f32 = 13.0;
    let t = angle * SLOTS / TAU;
    let slot = i32(floor(t));
    let phase = t - floor(t);
    if (hash3(slot, 9, 5) > 0.5) {
        return vec4<f32>(0.0);
    }
    let p = phase - hash3(slot, 8, 5) * 0.45;
    let env = sstep(0.0, 0.02, p) * (1.0 - sstep(0.05, 0.16, p));
    if (env <= 0.0) {
        return vec4<f32>(0.0);
    }
    let intensity = 0.45 + hash3(slot, 7, 5) * 1.0;
    let hx = hash3(slot, 1, 5) * 2.0 - 1.0;
    let hy = (hash3(slot, 2, 5) * 2.0 - 1.0) * 0.7;
    let radius = 0.05 + hash3(slot, 3, 5) * 0.13;
    let d = sqrt((sx - hx) * (sx - hx) + (sy - hy) * (sy - hy));
    let mag = env * intensity * sstep(radius, 0.0, d);
    let hue = hash3(slot, 4, 5);
    var col: vec3<f32>;
    if (hue < 0.42) {
        col = vec3<f32>(0.75, 0.83, 1.0);
    } else if (hue < 0.66) {
        col = vec3<f32>(0.82, 0.60, 1.0);
    } else if (hue < 0.85) {
        col = vec3<f32>(0.55, 0.95, 1.0);
    } else {
        col = vec3<f32>(1.0, 0.90, 0.66);
    }
    return vec4<f32>(col, mag);
}

// ---------------------------------------------------------------------------
// Surface shading — `planet_core::surface`. Returns (rgb, emissive) packed as
// a vec4.
// ---------------------------------------------------------------------------

fn surface(sx: f32, sy: f32, sz: f32, ofs: vec3<f32>, angle: f32) -> vec4<f32> {
    let px = sx + ofs.x;
    let py = sy + ofs.y;
    let pz = sz + ofs.z;
    var col: vec3<f32>;
    var emis = 0.0;

    if (ct.base == B_TERRESTRIAL) {
        var octaves = 6u;
        if (ct.ridged) { octaves = 5u; }
        let raw = fbm(px * ct.freq, py * ct.freq, pz * ct.freq, octaves);
        var n = raw;
        if (ct.ridged) { n = 1.0 - abs(2.0 * raw - 1.0); }
        let h = contrastf(n, ct.contrast);
        col = ramp_col(h);
        let cap = sstep(0.72, 0.9, abs(sy)) * ct.caps;
        col = lerp3(col, vec3<f32>(0.92, 0.95, 1.0), cap);
    } else if (ct.base == B_CRATERED) {
        let m = sstep(0.4, 0.6, fbm(px * 1.2, py * 1.2, pz * 1.2, 5u));
        let base_col = lerp3(ct.dark, ct.light, m);
        let w = worley(px * ct.freq, py * ct.freq, pz * ct.freq);
        let bowl = sstep(0.0, 0.35, w);
        let rim = sstep(0.30, 0.42, w) * (1.0 - sstep(0.42, 0.60, w));
        col = clamp01v(base_col * (0.55 + 0.45 * bowl) + vec3<f32>(rim * 0.30));
    } else if (ct.base == B_BANDED) {
        let flow = angle * 0.16 * sin(sy * ct.bands * 0.5);
        let warp = fbm_warp((px + flow) * 1.3, py * 1.3, pz * 1.3, 5u, 0.8);
        let lat = sy + (warp - 0.5) * ct.turb;
        let band = 0.5 + 0.5 * sin(lat * ct.bands);
        col = lerp3(ct.dark, ct.light, band);
        let fine = fbm((px + flow * 1.4) * 4.0, py * 4.0, pz * 4.0, 4u);
        col = lerp3(col, ct.light, sstep(0.55, 0.8, fine) * 0.35);
        if (ct.spot > 0.0) {
            col = great_spot(col, sx, sy, sz, angle, ct.spot);
        }
    } else if (ct.base == B_EMISSIVE) {
        let n = contrastf(fbm(px * ct.freq, py * ct.freq, pz * ct.freq, 6u), 1.7);
        let flow = fbm(px * 2.2 + angle * 0.7, py * 2.2, pz * 2.2 - angle * 0.5, 3u);
        let glow = clamp01(sstep(ct.glow_e0, ct.glow_e1, n) * (0.55 + 0.9 * flow));
        let mid = lerp3(ct.glow_lo, ct.glow_hi, 0.5);
        let gcol = cycle3(ct.glow_lo, mid, ct.glow_hi, n * 1.6 + angle * 0.12);
        col = lerp3(ct.rock, gcol, glow);
        emis = glow;
    } else {
        // B_CLOUDY
        let flow = (0.5 + 0.3 * cos(sy * 3.0)) * sin(angle);
        let t = fbm_warp((px + flow) * 2.0, py * 2.0, pz * 2.0, 5u, 0.7);
        let band = 0.5 + 0.5 * sin(sy * ct.bands + (t - 0.5) * 6.0 * ct.turb);
        col = lerp3(ct.dark, ct.light, clamp01(band * 0.6 + t * 0.4));
    }

    if (ct.aurora > 0.0) {
        let a = aurora_glow(sx, sy, sz, angle) * ct.aurora;
        let ac = cycle3(
            vec3<f32>(0.25, 0.95, 0.45),
            vec3<f32>(0.35, 0.85, 0.95),
            vec3<f32>(0.65, 0.40, 1.0),
            sy * 1.4 + angle * 0.1,
        );
        col = clamp01v(col + ac * a);
        emis = max(emis, a * 0.85);
    }
    if (ct.lightning > 0.0) {
        let lf = lightning_flash(sx, sy, angle);
        let f = lf.w * ct.lightning;
        col = clamp01v(col + lf.xyz * f);
        emis = max(emis, f);
    }
    return vec4<f32>(col, emis);
}

fn star_bg(ix: u32, iy: u32, seed: u32) -> vec3<f32> {
    let h = hash3(bitcast<i32>(ix), bitcast<i32>(iy), bitcast<i32>(seed));
    if (h > 0.986) {
        // Rust casts to u8, which truncates; the value is always positive here.
        let b = floor(150.0 + 105.0 * (h - 0.986) / 0.014);
        return vec3<f32>(b) / 255.0;
    }
    return vec3<f32>(9.0, 8.0, 20.0) / 255.0;
}

// ---------------------------------------------------------------------------
// Pixel-art output — `planet_core::finalize`. The limited palettes are global
// style rather than per-type, so unlike TYPES they live here rather than
// arriving in the buffer.
// ---------------------------------------------------------------------------

const PAL_GAMEBOY = array<vec3<f32>, 4>(
    vec3<f32>(0.06, 0.22, 0.06), vec3<f32>(0.19, 0.38, 0.19),
    vec3<f32>(0.55, 0.67, 0.06), vec3<f32>(0.61, 0.75, 0.06),
);
const PAL_ICE = array<vec3<f32>, 5>(
    vec3<f32>(0.04, 0.08, 0.18), vec3<f32>(0.13, 0.26, 0.46), vec3<f32>(0.35, 0.55, 0.78),
    vec3<f32>(0.62, 0.80, 0.94), vec3<f32>(0.92, 0.98, 1.0),
);
const PAL_SUNSET = array<vec3<f32>, 5>(
    vec3<f32>(0.10, 0.05, 0.20), vec3<f32>(0.40, 0.12, 0.34), vec3<f32>(0.78, 0.26, 0.34),
    vec3<f32>(0.97, 0.55, 0.30), vec3<f32>(1.0, 0.86, 0.56),
);

fn finalize(o: vec3<f32>, bx: f32, palette: u32, dither: f32) -> vec3<f32> {
    if (palette == 0u) {
        return quant(o, bx, 22.0, dither);
    }
    var n = 5u;
    if (palette == 1u) { n = 4u; }
    let lum = clamp01(o.x * 0.3 + o.y * 0.59 + o.z * 0.11);
    let f = (lum + bx * 0.14) * (f32(n) - 1.0);
    let i = u32(clamp(f + 0.5, 0.0, f32(n) - 1.0));
    if (palette == 1u) { return PAL_GAMEBOY[i]; }
    if (palette == 2u) { return PAL_ICE[i]; }
    return PAL_SUNSET[i];
}

// ---------------------------------------------------------------------------
// Entry points
// ---------------------------------------------------------------------------

/// Fullscreen triangle; no vertex buffer, no index buffer. Draw 3 vertices.
@vertex
fn vs_main(@builtin(vertex_index) vi: u32) -> @builtin(position) vec4<f32> {
    let p = array<vec2<f32>, 3>(
        vec2<f32>(-1.0, -1.0),
        vec2<f32>(3.0, -1.0),
        vec2<f32>(-1.0, 3.0),
    );
    return vec4<f32>(p[vi], 0.0, 1.0);
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    let size = fr[FR_SIZE];
    let angle = fr[FR_ANGLE];
    let seed = bitcast<u32>(fr[FR_SEED]);
    let palette = u32(fr[FR_PALETTE]);
    let dither = fr[FR_DITHER];
    let want_moons = fr[FR_MOONS] != 0.0;
    load_type(u32(fr[FR_TYPE]));

    // `pos` is at pixel centres, so it already carries the +0.5 the CPU loop
    // adds by hand: nx = (ix + 0.5 - cx) / rad.
    let ix = u32(pos.x);
    let iy = u32(pos.y);
    let c = size / 2.0;
    // 0.375 of the frame — leaves orbital margin for moons and rings.
    let rad = (size * 24.0 / 64.0) * ct.radius_scale;
    let nx = (pos.x - c) / rad;
    let ny = (c - pos.y) / rad;
    let d2 = nx * nx + ny * ny;

    let ofs = seed_offsets(seed);
    let l = normalize(vec3<f32>(-0.55, 0.45, 0.70)); // the hero key light
    let sina = sin(angle);
    let cosa = cos(angle);
    let has_atmo = any(ct.atmo != vec3<f32>(0.0));
    let RING_SQUASH = 0.38;
    let si = bitcast<i32>(seed);

    var o: vec3<f32>;

    if (d2 <= 1.0) {
        let nz = sqrt(1.0 - d2);
        let sx = nx * cosa + nz * sina;
        let sy = ny;
        let sz = -nx * sina + nz * cosa;

        let surf = surface(sx, sy, sz, ofs, angle);
        var col = surf.xyz;
        let emis = surf.w;

        if (ct.clouds > 0.0) {
            let cs = sin(angle * 2.0);
            let cc = cos(angle * 2.0);
            var cx3 = nx * cc + nz * cs + ofs.x;
            var cz3 = -nx * cs + nz * cc + ofs.z;
            let morph = sin(angle) * 0.6;

            if (ct.storm_cells > 0.0) {
                for (var k = 0; k < 2; k = k + 1) {
                    let vx = (hash3(si, k * 7 + 1, 3) * 2.0 - 1.0) * 1.6 + ofs.x;
                    let vz = (hash3(si, k * 7 + 2, 3) * 2.0 - 1.0) * 1.6 + ofs.z;
                    let dx = cx3 - vx;
                    let dz = cz3 - vz;
                    let fall = exp(-(dx * dx + dz * dz) * 2.2);
                    let sw = fall * sin(angle * 0.6) * 1.6 * ct.storm_cells;
                    let ss = sin(sw);
                    let sc = cos(sw);
                    cx3 = vx + dx * sc - dz * ss;
                    cz3 = vz + dx * ss + dz * sc;
                }
            }

            let cloud = fbm_warp(cx3 * 2.8, ny * 2.8 + ofs.y + morph, cz3 * 2.8 + morph, 4u, 0.9);
            // Shadow samples the cheap plain density, offset along the light.
            let dens = fbm(
                (cx3 + l.x * 0.45) * 2.8,
                ny * 2.8 + ofs.y + morph,
                (cz3 + l.z * 0.45) * 2.8 + morph,
                4u,
            );
            let sh = 1.0 - 0.22 * sstep(0.55, 0.72, dens) * ct.clouds;
            col = col * sh;
            col = lerp3(col, vec3<f32>(1.0), sstep(0.52, 0.70, cloud) * ct.clouds);
        }

        let diff = max(nx * l.x + ny * l.y + nz * l.z, 0.0);
        let shade = max(0.10 + 0.90 * diff, emis);
        o = col * shade;

        if (ct.specular > 0.0) {
            let hm = sqrt(l.x * l.x + l.y * l.y + (l.z + 1.0) * (l.z + 1.0));
            let ndh = max(nx * l.x / hm + ny * l.y / hm + nz * (l.z + 1.0) / hm, 0.0);
            let alb = col.x * 0.3 + col.y * 0.59 + col.z * 0.11;
            let mat = 1.0 - ct.spec_albedo * (1.0 - alb);
            let shimmer = 0.82 + 0.18 * fbm(sx * 5.0 + angle * 2.5, sy * 5.0, sz * 5.0, 2u);
            let sp = pow(ndh, ct.shininess) * ct.specular * mat * shimmer;
            o = clamp01v(o + vec3<f32>(sp));
        }
        if (has_atmo) {
            let rim = pow(1.0 - nz, 3.0) * 0.6;
            o = clamp01v(o + ct.atmo * rim);
        }
    } else {
        o = star_bg(ix, iy, seed);
    }

    if (ct.rings) {
        let rr = sqrt(nx * nx + (ny / RING_SQUASH) * (ny / RING_SQUASH));
        if (rr >= ct.ring_inner && rr <= ct.ring_outer && (ny < 0.0 || d2 > 1.0)) {
            let rn = (rr - ct.ring_inner) / (ct.ring_outer - ct.ring_inner);
            let stripes = 0.5 + 0.5 * sin(rn * 36.0);
            var alpha = clamp01(0.30 + 0.55 * stripes);
            if (rn > 0.46 && rn < 0.54) {
                alpha = alpha * 0.12;
            }
            let rb = 0.55 + 0.45 * stripes;
            // The hero framing's backdrop is always opaque, so the ring blends
            // in place — there is no transparent-tile case here.
            o = lerp3(o, ct.ring_col * rb, alpha);
        }
    }

    // Crisp dark rim, before moons so a front moon crosses over it.
    if (d2 <= 1.0) {
        let edge = 1.0 - 1.3 / rad;
        if (d2 > edge * edge) {
            o = o * vec3<f32>(0.26, 0.26, 0.30);
        }
    }

    if (want_moons) {
        // Orbits are re-derived per pixel rather than passed in: it is a handful
        // of hashes next to the fbm above, and it keeps the moon layout owned by
        // one place instead of a JS copy of hash3.
        let count = min(u32(hash3(si, 50, 1) * 2.6), 2u);
        for (var k = 0u; k < count; k = k + 1u) {
            let ks = i32(k) * 5;
            let orbit = 1.16 + hash3(si, ks + 1, 2) * 0.14;
            let tilt = 0.34 + hash3(si, ks + 2, 2) * 0.30;
            let speed = 0.25 + hash3(si, ks + 3, 2) * 0.4;
            let phase = hash3(si, ks + 4, 2) * TAU;
            let mr = (0.12 + hash3(si, ks + 5, 2) * 0.09) * max(ct.radius_scale, 0.6);
            let oa = angle * speed + phase;
            let mx = cos(oa) * orbit;
            let my = sin(oa) * orbit * tilt;
            let depth = sin(oa);
            let ms = f32(k) + 1.0;

            let ld2 = (nx - mx) * (nx - mx) + (ny - my) * (ny - my);
            if (ld2 < mr * mr && (depth > 0.0 || d2 > 1.0)) {
                let lnx = (nx - mx) / mr;
                let lny = (ny - my) / mr;
                let lnz = sqrt(max(1.0 - lnx * lnx - lny * lny, 0.0));
                let mdiff = max(lnx * l.x + lny * l.y + lnz * l.z, 0.0);
                let mrim = 0.4 + 0.6 * sstep(0.0, 0.26, lnz);
                let msh = (0.12 + 0.9 * mdiff) * mrim;
                let t = fbm(lnx * 3.0 + ms * 9.0, lny * 3.0, ms * 5.0, 2u);
                let mbase = lerp3(vec3<f32>(0.30, 0.29, 0.33), vec3<f32>(0.60, 0.59, 0.62), sstep(0.4, 0.6, t));
                o = mbase * msh;
            }
        }
    }

    let px = clamp01v(finalize(o, bayer(ix, iy), palette, dither));
    // The CPU path writes `(v * 255.0) as u8`, which truncates. Writing to an
    // rgba8unorm target instead rounds to nearest, which would put ~58% of
    // pixels one byte above the CPU's. Pre-truncate so the target's rounding
    // has an exact integer to land on and the two paths agree.
    return vec4<f32>(floor(px * 255.0) / 255.0, 1.0);
}
