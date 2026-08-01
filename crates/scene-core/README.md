# scene-core

The scene-compositor kit shared by every multi-body demo: a draggable `Camera`, a
seeded `Rng`, and the `Tile` + `blit` alpha compositor.

## Camera

A world→screen camera. Drag pans; zoom is about the viewport centre, which keeps
the scene and its parallax anchored no matter where you have panned to.

Zoom reveals detail rather than magnifying fixed pixels: the demos size their
render buffer so a rendered pixel is a constant on-screen block at every zoom,
and bodies render at a resolution that grows as you zoom in.

## Tile and blit

A body renders into a small RGBA `Tile` cut out on transparency, and `blit`
alpha-composites it. Two properties matter to callers:

- **The compositor walks runs, not pixels.** At the upscales a zoomed-in scene
  reaches, tens of consecutive destination pixels share one source pixel, so
  `blit` fetches the source, tests alpha and computes the blend factors once per
  run — and skips a transparent run without touching the destination. Compositing
  one full-screen body: 10.6 ms → **2.2 ms**.
- **`blit` maps destination back to source as `int((dd + 0.5) / scale)`.** The GPU
  fragment shaders use that same expression, which is what keeps `planet_pixel`,
  `sun_pixel` and the detail caps meaningful on both paths with no second render
  target. Change one, change both.

## visible_tile_rect: the clip is the visibility test

`visible_tile_rect` asks where a tile actually lands on screen and returns the
sub-rect `blit` will sample. Renderers take that rect and shade nothing else
(`render_tile_into`, `render_star_tile_into`). A disc twice the viewport height
has ~70% of its tile hanging off the edge, and that work simply stops happening.

Two consequences, both load-bearing:

- **An empty rect is the visibility test** — exact, and free. Do not add a "body
  radius × some margin" off-screen check beside it. There was one; it had to
  over-estimate for ringed giants and corona halos, so it kept rendering
  full-price tiles for bodies that were entirely off-screen.
- **Pixels outside the rect are not shaded**, so a tile is only valid for the
  placement its clip came from. Anything that caches a tile must put the clip in
  the cache key (`sun-core`'s `SunCache` does) and snap the rect outward to a grid
  (`snap_out`) — otherwise a camera drifting one pixel a frame invalidates the
  cache every frame and the caching buys nothing.

The rect is **exact**, not padded: `blit` reads tile pixel `map(dd)` for each
destination offset it visits and `map` is monotone, so the two endpoints bound the
set. Both functions share that expression for exactly that reason. A rect that
under-reports by a pixel leaves an unshaded seam visible only at zoom levels
nobody screenshots — which is what this crate's million-read sweep test is for.

## Rng

Seeded and deterministic. Same seed rebuilds the identical scene, forever.
