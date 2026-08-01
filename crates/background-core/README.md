# background-core

**The** backdrop — everything a scene paints *before* its bodies: a dithered navy
ground, an optional seeded nebula, and three parallax star layers. `solar`, `moon`,
`comet` and `asteroid` all paint through it.

Every scene calls `paint_backdrop` (ground + nebula) then `paint_stars`. A new
scene supplies a `Backdrop`, a `Starfield` const, and a closure mixing its seed
into the star grid — it does not write another star loop. The four that existed
before differed only in constants, which is how they silently drifted apart.

## Parallax that cannot run away

Each star layer is a fixed *screen-space* grid, scrolled by the camera's
**accumulated screen-space pan** (Δcam·zoom summed over time) at a fraction `p` of
the foreground. Two properties follow:

- On **pan**, stars move slower than the scene by the same ratio at every zoom —
  no runaway when zoomed out.
- On **zoom**, they do not move at all: pure zoom adds no screen displacement, and
  zoom is about the viewport centre.

So a star can never outrun the scene, and the on-screen count stays constant — no
wall, no swim. **Star density** and **star parallax** tune the count and the scroll
rate. The far layer and the nebula fade out (and are skipped) when you zoom onto a
body.

## The backdrop is cached, and a pan scrolls it

Profiling the uncached renderer showed the backdrop was ~50% of every frame and
O(pixels), yet almost entirely *stable*: it never depends on animation time, the
nebula scrolls at only 9% of pan and its shape is zoom-independent, and the base
navy is constant. Only the stars truly move, and they are a cheap O(cells) overlay.

Time-quantizing alone only made a *still* camera cheap — the moment you dragged,
every key changed and the whole backdrop was rebuilt. The fix is to stop treating a
pan as invalidation: the backdrop is a function of *where* the camera is, so a
panned frame is the previous frame **moved**. Both cached layers are kept as
sprites indexed by world position; a drag memmoves them and repaints only the strip
that scrolled into view.

- **fBm nebula field** (low-res), indexed by absolute world cell. A pan re-bakes
  ~1% of it; a pure **zoom re-bakes none**, because the fade is applied when the
  sprite is read rather than baked in.
- **Base navy + nebula layer** (full-res, all but stars and orbits) — the dominant
  cost of the two, by about 4:1. Scrolled the same way, so a drag costs a sliver of
  a screenful instead of a full composite.

Measured at 1000x640, a fast drag went **4.9 ms → 0.33 ms**, and the backdrop from
45% of a panning frame to 22%.

What makes the sprites slide is that the whole layer is a function of where the
*clouds* are — so the nebula's ordered dither is anchored to them rather than to
the screen, which also stops the stipple crawling as they drift. A scene that sets
`Backdrop::dither` under a nebula reintroduces a screen-pinned term and opts out of
the scrolled path; none currently does.

Both layers live in `BackdropCache`, so any scene that grows a nebula inherits
them. `solar` adds a third level on top (`render_system_cached`) that also caches
the orbit paths, so a still camera makes the entire backdrop one `memcpy`.

`cargo test -p background-core` pins the fast paths to the uncached renderer
byte-for-byte across a **scripted pan**. A scroll bug shows up as stale sky smeared
across a drag and in no still frame, so the test has to pan.

## Amdahl lives here

The backdrop is full-screen serial work that scales with window area, and `bg_key`
holds the camera — so a camera *following* a planet invalidates the cache every
frame and repaints it, in every CPU path, pooled or not. That is why "16 workers
changed nothing", and it is most of what moving to the GPU actually bought. See
the [root README](../../README.md#rendering-on-the-gpu).

## Scatter, don't gather

The first version of the GPU backdrop drew the stars in the *fragment* shader: each
pixel asked which of the nine surrounding cells, in each of three parallax layers,
might have placed a star on it. That is the only way a fragment can ask the
question, and it is 27 hashes per pixel across the whole screen — against
`paint_stars`, which walks the lit cells and plots one pixel each, roughly one hash
per fifty pixels. **A thousand times the work for the same picture.**

It was three quarters of the fragment cost. Under SwiftShader (a software
rasterizer, so these are CPU numbers — but the ratio is fragment ALU either way),
800x500, fit view:

| | ms/frame | fps |
|---|---:|---:|
| stars gathered in the fragment shader | 216.6 | 4.6 |
| **stars scattered as point sprites** | **50.0** | **20.0** |

With the stars as points, switching the starfield off entirely changes the frame by
*nothing measurable* — the check that says the cost really moved rather than merely
shrank.

The fix reuses what the orbit paths already did: **`visit_stars` is the one cell
walk**, `paint_stars` plots pixels from it and `gl_star_points` emits vertices from
it, both into the same `(x, y, r, g, b)` buffer under an additive blend. The lesson
generalizes past this repo — when porting a scatter to a shader, look for the
vertex path before you write the gather.

The nebula is the same shape of problem, still unfixed: `BackdropCache` bakes one
fBm sample per 8x8 cell and scrolls the sprite, where `src/backdrop.glsl`
recomputes that per-cell value at every pixel — 64x the noise evaluations. Far
milder than the stars' 1000x, and not the bottleneck now. If it becomes one, port
the sprite rather than thin the shader: bake the cell field into a low-res texture,
scroll it, repaint only the exposed strip, and let hardware sampling do the rest.

That difference is also why `verify-gl.mjs --demo solar` reports a raw
disagreement rate of 17–30%: the GPU evaluates the cloud fBm per *pixel* where the
CPU bakes it per *cell*, so the two round differently right at the density
threshold. At a zoom where the clouds have faded, the backdrop is byte-exact.
