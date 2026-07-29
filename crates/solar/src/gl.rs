//! The scene's WebGL2 frame packer — see the module doc-comment in lib.rs for
//! why this is not compiled into the native build.
//!
//! Where `draw_bodies` shades a tile per body and blits it, the GPU draws one
//! screen-space quad per body and lets the rasterizer do the coverage. So what
//! this module produces is not pixels but a *draw list*: the backdrop's
//! uniforms, the orbit paths' vertices, and one record per body in back-to-front
//! order. Every number in it comes from the same expressions `draw_bodies_band`
//! uses — `Planet::at`, `to_screen`, `dest_rect`, the detail caps, the sunward
//! light — so the GPU scene is the CPU scene, drawn by a different machine.
//!
//! Three things the CPU path carries that are simply absent here, all of them
//! caches for work a GPU does not mind repeating:
//!
//!   * `BackdropCache` — the scrolling ground/nebula sprite. One fullscreen
//!     triangle replaces it, so a camera that follows a planet (which
//!     invalidates the key every frame, and is exactly the case the pool could
//!     not help) costs the same as a still one.
//!   * `SunCache` and its quantized boil clock. The clock was quantized so a
//!     costly bake could be reused; with no bake, `t_sun` is passed through and
//!     the convection stops stepping.
//!   * `visible_tile_rect`. An off-screen quad is clipped by the rasterizer for
//!     free, and a partly-visible one shades only the fragments it covers —
//!     which is what the clip rect was arranging by hand.

use super::*;

/// Floats of header in front of each body record — see [`gl_bodies`].
pub const GL_BODY_HEADER: usize = 8;
/// Floats per body record: the header, then the shader's own uniform block.
///
/// Sized for the larger of the two bodies. A star needs 32 slots against a
/// planet's 160, and one stride keeps the records addressable by index.
pub const GL_BODY_STRIDE: usize = GL_BODY_HEADER + planet_core::GL_UNIFORMS_LEN;

/// `kind` in a body record's header.
pub const GL_KIND_STAR: f32 = 0.0;
/// `kind` in a body record's header.
pub const GL_KIND_PLANET: f32 = 1.0;

/// Most bodies a record buffer must hold: the star plus the hard planet cap in
/// [`System::generate_n`].
pub const GL_MAX_BODIES: usize = 17;

/// The star-grid salt for this system.
///
/// Mixed into the hash's third axis rather than added to the cell coordinates:
/// offsetting the grid would give every system the SAME sky panned sideways. The
/// 977 stride clears the three layer salts. One definition, because
/// `paint_background`'s closure and the GPU's `u_skySalt` must agree or the two
/// renderers draw different constellations.
pub fn sky_salt(seed: u32) -> i32 {
    seeded_sky_salt(seed)
}

/// This frame's starfield, with the zoom fades `paint_background` applies.
fn sky_of(sys: &System, zoom: f32) -> Starfield<'static> {
    Starfield {
        layers: STAR_LAYERS,
        tints: STAR_TINTS,
        density: sys.star_density,
        pan_scale: sys.star_parallax,
        far_fade: 1.0 - smoothstep(3.0, 9.0, zoom),
    }
}

/// Fill `out` with the backdrop shader's uniforms — ground and nebula. The stars
/// are a separate pass ([`gl_star_points`]).
pub fn gl_backdrop(sys: &System, cam: &Camera, bgx: f32, bgy: f32, out: &mut [f32]) {
    let neb_amt = 1.0 - smoothstep(2.5, 7.0, cam.zoom);
    background_core::gl_uniforms(&BACKDROP, sys.seed, bgx, bgy, sys.star_parallax, neb_amt, out);
}

/// Write the visible stars as point sprites; returns how many.
///
/// Scattered from the same walk `paint_stars` uses, salted the same way. The
/// alternative — having the fragment shader gather, i.e. ask at every pixel
/// which of nine cells per layer might have lit it — measured as HALF the GPU
/// frame on a full-screen backdrop, for 27 hashes per pixel against roughly one
/// per fifty here.
pub fn gl_star_points(sys: &System, w: u32, h: u32, cam: &Camera, bgx: f32, bgy: f32, out: &mut [f32]) -> usize {
    let salt0 = sky_salt(sys.seed);
    background_core::gl_star_points(
        &sky_of(sys, cam.zoom), w, h, bgx, bgy,
        move |cx, cy, salt| hash3(cx, cy, salt0.wrapping_add(17 + salt)),
        out,
    )
}

/// Screen-space `(x, y)` for every dot of every dashed orbit path, written into
/// `out` as consecutive pairs; returns how many points were written.
///
/// The dashes and the sampling are `paint_orbit`'s, and the ellipse itself comes
/// from `Planet::plane_point` — uploading points rather than re-deriving Kepler's
/// equation in a vertex shader is what keeps that math in one place. At 220 steps
/// over at most 16 planets this is a few KB a frame against the tens of MB of
/// pixels the CPU path was moving.
///
/// The coordinates are the pixel *centres* the CPU stamps at, so a point sprite
/// of `orbit_width` px lands on the same pixels. Same `(x, y, r, g, b)` layout as
/// [`gl_star_points`], so one vertex buffer and one program serve both.
pub fn gl_orbit_points(sys: &System, w: u32, h: u32, cam: &Camera, out: &mut [f32]) -> usize {
    // `paint_orbit`'s faint additive blue-grey.
    const DOT: [f32; 3] = [26.0 / 255.0, 30.0 / 255.0, 40.0 / 255.0];
    let steps = 220;
    let stride = background_core::GL_POINT_STRIDE;
    let mut n = 0;
    for p in &sys.planets {
        let e = p.ecc(sys.ecc);
        for k in 0..steps {
            if (k / 3) % 2 == 0 {
                continue; // dashed: skip every few samples
            }
            if (n + 1) * stride > out.len() {
                return n;
            }
            let ea = TAU * k as f32 / steps as f32;
            let (x1, y1) = p.plane_point(ea, e);
            let wx = x1 * sys.spacing;
            let wy = y1 * ORBIT_FLATTEN * p.tilt * sys.spacing;
            let (sx, sy) = to_screen(wx, wy, cam, w, h);
            // `sx as i32` in the Rust, then the stamp is centred on that pixel.
            out[n * stride] = (sx as i32) as f32 + 0.5;
            out[n * stride + 1] = (sy as i32) as f32 + 0.5;
            out[n * stride + 2..n * stride + 5].copy_from_slice(&DOT);
            n += 1;
        }
    }
    n
}

/// Fill `out` with one [`GL_BODY_STRIDE`]-float record per visible body, sorted
/// back-to-front; returns how many were written.
///
/// Each record is `[kind, x0, y0, edge, tile size, _, _, _]` — the quad to draw
/// in screen px, from the same `dest_rect` the compositor uses — followed by that
/// shader's uniform block. A caller walks them in order, binds the program named
/// by `kind`, and draws a quad; painter's algorithm over alpha blending gives the
/// same depth sort `blit` gave.
///
/// Bodies too small to see are skipped, exactly as on the CPU. Off-screen ones
/// are NOT skipped here — an off-screen quad is clipped by the rasterizer before
/// it shades anything, which is cheaper than the test.
#[allow(clippy::too_many_arguments)]
pub fn gl_bodies(
    sys: &System,
    w: u32,
    h: u32,
    cam: &Camera,
    t_orbit: f32,
    t_spin: f32,
    t_sun: f32,
    out: &mut [f32],
) -> usize {
    // Same draw list `draw_bodies_band` builds: the sun at depth 0, planets
    // sorted around it by their orbital depth.
    let mut order = sys.order.borrow_mut();
    order.clear();
    order.push((0.0, -1));
    for (i, p) in sys.planets.iter().enumerate() {
        let (_, _, depth) = p.at(t_orbit, sys.spacing, sys.ecc);
        order.push((depth, i as i32));
    }
    order.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));

    let (suncx, suncy) = to_screen(0.0, 0.0, cam, w, h);
    let buf_cap = w.max(h) as f32 * 0.6;
    let maxr = buf_cap.min(sys.planet_detail);
    let maxr_sun = buf_cap.min(sys.sun_detail);

    let mut n = 0;
    for &(_, which) in order.iter() {
        if (n + 1) * GL_BODY_STRIDE > out.len() {
            break;
        }
        let rec = &mut out[n * GL_BODY_STRIDE..(n + 1) * GL_BODY_STRIDE];
        rec.fill(0.0);
        let (head, body) = rec.split_at_mut(GL_BODY_HEADER);

        // Per-body pixelation: shade at a smaller radius, then let the quad
        // magnify it by the same factor — the body keeps its on-screen size and
        // turns blockier, which is what `blit` was doing with a scale.
        let (kind, cx, cy, rad_px, pixel, maxr) = if which < 0 {
            (GL_KIND_STAR, suncx, suncy, sys.sun_radius * sys.sun_size * cam.zoom, sys.sun_pixel, maxr_sun)
        } else {
            let p = &sys.planets[which as usize];
            let (wx, wy, _) = p.at(t_orbit, sys.spacing, sys.ecc);
            let (sx, sy) = to_screen(wx, wy, cam, w, h);
            (GL_KIND_PLANET, sx, sy, p.radius * sys.planet_size * cam.zoom, sys.planet_pixel, maxr)
        };
        if rad_px < 0.5 {
            continue;
        }
        let rad_render = (rad_px / pixel).clamp(2.0, maxr);
        let scale = rad_px / rad_render;

        let size = if which < 0 {
            // `t_sun` raw, not snapped to `SUN_TQUANT`: that quantum exists so a
            // costly tile bake can be reused between frames, and there is no
            // bake here, so the convection runs continuously instead of stepping.
            let size = sun_core::star_tile_size(rad_render, CORONA_REACH);
            let (x0, y0) = dest_origin(cx, cy, size, scale);
            head[1..4].copy_from_slice(&[x0 as f32, y0 as f32, (size as f32 * scale).round().max(1.0)]);
            sun_core::gl_uniforms(
                &SUNS[sys.sun_kind], sys.seed, t_sun, rad_render, CORONA_REACH, true, x0, y0, scale, body,
            )
        } else {
            let p = &sys.planets[which as usize];
            // Light comes from the sun: direction from planet toward the star, in
            // screen space (+x right, +y up), biased toward the viewer so the
            // terminator sits pleasingly rather than dead edge-on.
            let (dx, dy) = (suncx - cx, suncy - cy);
            let lmag = (dx * dx + dy * dy).sqrt().max(1e-3);
            let (lx, ly) = (dx / lmag, -dy / lmag); // screen y is down → flip
            let lz = 0.55;
            let m = (lx * lx + ly * ly + lz * lz).sqrt();
            let light = [lx / m, ly / m, lz / m];
            // `spin_a` both turns the surface and advances that world's weather.
            let spin_a = p.phase + p.spin * t_spin * TAU;
            let size = planet_core::tile_size(p.ptype, rad_render);
            let (x0, y0) = dest_origin(cx, cy, size, scale);
            head[1..4].copy_from_slice(&[x0 as f32, y0 as f32, (size as f32 * scale).round().max(1.0)]);
            // The GPU runs the LIVE shader — `F_BAKED_*` buy CPU time at the
            // price of frozen weather, so `frozen_clouds` has nothing to switch.
            planet_core::gl_tile_uniforms(
                p.ptype, p.seed, spin_a, light, rad_render, planet_core::F_ALL, x0, y0, scale, body,
            )
        };
        head[0] = kind;
        head[4] = size as f32;
        n += 1;
    }
    n
}

/// `scene_core::dest_rect`'s origin, which is private to that crate — the quad
/// has to land on the same pixel the blit would have started at, and the shader
/// maps back through it to find its tile pixel.
fn dest_origin(sx: f32, sy: f32, tile_size: u32, scale: f32) -> (i32, i32) {
    let edge = (tile_size as f32 * scale).round().max(1.0);
    ((sx - edge * 0.5).floor() as i32, (sy - edge * 0.5).floor() as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The draw list has to place bodies where `draw_bodies` places them, or the
    /// two renderers disagree about the whole scene rather than about shading.
    /// Checked against the compositor's own `visible_tile_rect`, which is derived
    /// from the same `dest_rect` this reimplements.
    #[test]
    fn body_quads_land_where_the_compositor_puts_them() {
        let sys = System::generate(7);
        let (w, h) = (320u32, 200u32);
        let cam = Camera { x: 0.0, y: 0.0, zoom: 1.4 };
        let mut buf = vec![0.0; GL_MAX_BODIES * GL_BODY_STRIDE];
        let n = gl_bodies(&sys, w, h, &cam, 0.3, 0.2, 0.1, &mut buf);
        assert!(n >= 2, "expected the star and some planets, got {n}");

        let (suncx, suncy) = to_screen(0.0, 0.0, &cam, w, h);
        for i in 0..n {
            let r = &buf[i * GL_BODY_STRIDE..];
            let (x0, y0, edge, size) = (r[1], r[2], r[3], r[4] as u32);
            assert!(size >= 6, "tile {i} is {size} px");
            // The quad is exactly the destination rect: `round(size * scale)`
            // square, with its origin floored off the body's centre.
            assert_eq!(edge, edge.round(), "quad {i} edge is not whole");
            assert!(edge >= 1.0);
            if r[0] == GL_KIND_STAR {
                let scale = edge / size as f32;
                assert_eq!((x0, y0), {
                    let d = dest_origin(suncx, suncy, size, scale);
                    (d.0 as f32, d.1 as f32)
                });
            }
            assert!(r.iter().take(GL_BODY_STRIDE).all(|v| v.is_finite()), "record {i}");
        }
    }

    /// A record buffer sized for the cap must never be overrun, and a short one
    /// must truncate rather than panic.
    #[test]
    fn the_draw_list_respects_its_buffer() {
        let mut sys = System::generate_n(3, 16);
        sys.set_view(1.0, 1.0, 1.0, 1.0, 1.0, 160.0, 110.0, 0.5, 1.0);
        let mut full = vec![0.0; GL_MAX_BODIES * GL_BODY_STRIDE];
        let n = gl_bodies(&sys, 400, 300, &Camera::centered(), 0.0, 0.0, 0.0, &mut full);
        assert_eq!(n, 17, "16 planets and a star must all fit in GL_MAX_BODIES");

        let mut small = vec![0.0; 3 * GL_BODY_STRIDE];
        assert!(gl_bodies(&sys, 400, 300, &Camera::centered(), 0.0, 0.0, 0.0, &mut small) <= 3);
    }

    /// The orbit dots have to be the ones `paint_orbit` stamps — same dash
    /// pattern, same ellipse, same pixel.
    #[test]
    fn orbit_points_match_the_painted_path() {
        let sys = System::generate(21);
        let (w, h) = (300u32, 180u32);
        let cam = Camera { x: 12.0, y: -4.0, zoom: 0.8 };
        let st = background_core::GL_POINT_STRIDE;
        let mut pts = vec![0.0; 16 * 220 * st];
        let n = gl_orbit_points(&sys, w, h, &cam, &mut pts);
        // 220 samples through the `(k / 3) % 2` dash. Counted rather than
        // halved: 220 is not a whole number of dash periods, so the last group
        // is a stub and "half" is 109, not 110.
        let per = (0..220u32).filter(|k| (k / 3) % 2 != 0).count();
        assert_eq!(n, sys.planets.len() * per);

        let p = &sys.planets[0];
        let e = p.ecc(sys.ecc);
        let mut k = 0;
        for step in 0..220u32 {
            if (step / 3) % 2 == 0 {
                continue;
            }
            let (x1, y1) = p.plane_point(TAU * step as f32 / 220.0, e);
            let (sx, sy) = to_screen(x1 * sys.spacing, y1 * ORBIT_FLATTEN * p.tilt * sys.spacing, &cam, w, h);
            assert_eq!(pts[k * st], (sx as i32) as f32 + 0.5, "dot {k} x");
            assert_eq!(pts[k * st + 1], (sy as i32) as f32 + 0.5, "dot {k} y");
            k += 1;
        }
    }

    /// One definition of the salt, or the two renderers draw different skies.
    #[test]
    fn the_sky_salt_is_what_the_cpu_closure_uses() {
        for seed in [0u32, 1, 7, 0xdead_beef] {
            assert_eq!(sky_salt(seed), (seed as i32).wrapping_mul(977));
        }
    }
}
