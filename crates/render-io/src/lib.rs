//! render-io — the native `image`-crate orchestration shared by the space-crate
//! bins: turning RGBA frames into GIFs, contact-sheet PNGs, and orbit/pan
//! animations. Two families:
//!   * "spinning body" (planet, star): [`write_spin_gif`], [`write_spin_grid_gif`],
//!     [`write_contact_sheet`].
//!   * "scene / camera" (solar, moon, comet, asteroid): [`fit_zoom`],
//!     [`write_orbit_gif`], [`write_anim_gif`], [`write_poster`].
//!
//! The per-frame render call and the (per-crate) `Camera` stay in each bin's
//! closure; this crate only owns the `image`-crate calls. Every helper issues
//! those calls in the exact order/params the bins used to, so output bytes are
//! unchanged (e.g. angle/`t` keep the `X * f as f32 / n as f32` associativity,
//! and each frame gets a fresh buffer).
//!
//! The spinning-body family runs on [`parallel_map`], a tokio-backed render
//! pool — both its sprite renders and its GIF palette reduction. Sprite math is
//! pure (a `(type, seed, angle)` triple always yields the same pixels, and
//! nothing in `planet-core`/`star` is shared or mutable) and GIF palettes are
//! per-frame, so evaluating both concurrently and reassembling in index order
//! is byte-for-byte what the serial loops produced — verified against every
//! file in `out/`. The scene family is deliberately left serial; see the note
//! above [`encode_gif`].

use image::imageops;
use std::cell::Cell;
use std::error::Error;
use std::f32::consts::TAU;
use std::fs::File;
use std::sync::{Arc, OnceLock};

// Re-exported so bins need no direct `image` dependency.
pub use image::{Rgba, RgbaImage};

type Res = Result<(), Box<dyn Error>>;

// ---------------------------------------------------------------------------
// The render pool
// ---------------------------------------------------------------------------

/// The process-wide render runtime, built once on first use.
///
/// Rendering is pure compute — no I/O, nothing to await — so every job is a
/// `spawn_blocking` call and the async side does nothing but collect joins.
/// That is why the runtime is *current-thread*: a work-stealing scheduler
/// could only ever host one non-yielding future per worker, so it would buy
/// nothing over the blocking pool. `max_blocking_threads` is therefore where
/// the actual parallelism is set, and it is pinned to the core count —
/// compute-bound work gains nothing from oversubscription, and tokio's 512
/// default would thrash the cache badly.
fn pool() -> &'static tokio::runtime::Runtime {
    static POOL: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    POOL.get_or_init(|| {
        let threads = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(1);
        tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(threads)
            .thread_name("render-io")
            .build()
            .expect("current-thread runtime builds without I/O drivers")
    })
}

thread_local! {
    /// Set on a pool thread for as long as it is running a render job. See
    /// [`parallel_map`] for why the nesting it guards would deadlock.
    static ON_POOL: Cell<bool> = const { Cell::new(false) };
}

/// Marks the current thread as busy with a render job, and — crucially — clears
/// the mark on unwind too: tokio catches a blocking job's panic and *reuses*
/// the thread, so a leaked flag would silently serialize every later job that
/// landed on it.
struct OnPool;

impl OnPool {
    fn enter() -> Self {
        ON_POOL.with(|f| f.set(true));
        OnPool
    }
}

impl Drop for OnPool {
    fn drop(&mut self) {
        ON_POOL.with(|f| f.set(false));
    }
}

/// Evaluate `job(0..n)` across the render pool and return the results **in
/// index order**, so callers can hand them straight to a frame encoder.
///
/// Nested calls run serially. The pool is bounded at the core count, so an
/// outer job that parked a thread waiting on inner jobs could hold every slot
/// while none of the inner work has a slot to run in — a classic bounded-pool
/// deadlock. Collapsing the inner level is also the right split of the work:
/// whichever level is outermost already has enough jobs to fill the pool.
/// This is what lets a bin fan out over *whole GIFs* and still call helpers
/// that fan out over frames.
pub fn parallel_map<T, F>(n: usize, job: F) -> Vec<T>
where
    F: Fn(usize) -> T + Send + Sync + 'static,
    T: Send + 'static,
{
    parallel_map_each((0..n).collect(), job)
}

/// [`parallel_map`] over owned inputs: job `i` gets `items[i]` by value, so a
/// job can mutate its own input without the jobs sharing anything. Same
/// ordering, same nesting rule.
fn parallel_map_each<I, T, F>(items: Vec<I>, job: F) -> Vec<T>
where
    I: Send + 'static,
    F: Fn(I) -> T + Send + Sync + 'static,
    T: Send + 'static,
{
    if items.len() <= 1 || ON_POOL.with(Cell::get) {
        return items.into_iter().map(&job).collect();
    }
    let n = items.len();
    let job = Arc::new(job);
    pool().block_on(async move {
        let tasks: Vec<_> = items
            .into_iter()
            .map(|item| {
                let job = Arc::clone(&job);
                tokio::task::spawn_blocking(move || {
                    let _on_pool = OnPool::enter();
                    job(item)
                })
            })
            .collect();
        // Awaited in spawn order, so a panicking job surfaces as the same
        // failure it would have been serially rather than as a lost result.
        let mut out = Vec::with_capacity(n);
        for t in tasks {
            out.push(t.await.expect("render job panicked"));
        }
        out
    })
}

/// Nearest-neighbour integer upscale (the bins' old `zoom`).
pub fn upscale_nearest(img: &RgbaImage, s: u32) -> RgbaImage {
    imageops::resize(img, img.width() * s, img.height() * s, imageops::FilterType::Nearest)
}

// GIF encoding is two very unequal halves: reducing each frame to a 256-colour
// palette (NeuQuant — the dominant cost of these bins, several times the sprite
// math), and appending the result to one LZW stream. Only the second half is
// stateful. Palettes are per-frame in this format and `from_rgba_speed` reads
// nothing but its own pixels, so building them on the pool and writing them in
// frame order yields the identical stream.
//
// Both encoders drive `gif` directly rather than `image`'s `GifEncoder`, which
// welds the halves together in one `encode_frame` call. Everything the wrapper
// did is reproduced: speed 1 (`GifEncoder::new`'s default), dimensions from the
// first frame, infinite repeat set before the first write, `Background`
// disposal, and its centisecond delay rounding — verified byte-for-byte against
// the old path on every GIF in `out/`.
//
// They stay two functions rather than one with a flag because the scene bins
// must not gain a tokio call in their inlining scope: this workspace's float
// math is famously LTO-sensitive (see CLAUDE.md), and routing solar/moon/comet/
// asteroid through the pool shifts comet's dithered tail by a quantization
// level. Those bins were not in scope, so their encode path is left untouched.

/// One frame's palette reduction — the parallelizable half, and pure.
fn quantize(img: RgbaImage, w: u16, h: u16, delay: u16) -> gif::Frame<'static> {
    let mut px = img.into_raw();
    let mut frame = gif::Frame::from_rgba_speed(w, h, &mut px, 1);
    frame.delay = delay;
    frame.dispose = gif::DisposalMethod::Background;
    frame
}

/// Frame geometry + `image`'s delay rounding: whole-millisecond ratio truncated
/// to centiseconds, saturating rather than erroring on overflow.
fn gif_params(frames: &[RgbaImage], delay_ms: u32) -> Result<Option<(u16, u16, u16)>, Box<dyn Error>> {
    let Some(f) = frames.first() else { return Ok(None) };
    let delay = u16::try_from(delay_ms / 10).unwrap_or(u16::MAX);
    Ok(Some((u16::try_from(f.width())?, u16::try_from(f.height())?, delay)))
}

/// The serial writer: appends already-quantized frames as an infinite-repeat GIF.
fn write_gif(path: &str, w: u16, h: u16, frames: Vec<gif::Frame<'static>>) -> Res {
    let file = File::create(path)?;
    let mut enc = gif::Encoder::new(file, w, h, &[])?;
    enc.set_repeat(gif::Repeat::Infinite)?;
    for frame in &frames {
        enc.write_frame(frame)?;
    }
    Ok(())
}

/// Encode an ordered sequence of frames as an infinite-repeat GIF. Serial —
/// used by the scene family.
fn encode_gif<I: IntoIterator<Item = RgbaImage>>(path: &str, delay_ms: u32, frames: I) -> Res {
    let frames: Vec<RgbaImage> = frames.into_iter().collect();
    let Some((w, h, delay)) = gif_params(&frames, delay_ms)? else { return Ok(()) };
    let quantized = frames.into_iter().map(|img| quantize(img, w, h, delay)).collect();
    write_gif(path, w, h, quantized)
}

/// [`encode_gif`] with the palette reduction fanned out over the pool — used by
/// the spinning-body family, where it is the bulk of the wall clock.
fn encode_gif_parallel(path: &str, delay_ms: u32, frames: Vec<RgbaImage>) -> Res {
    let Some((w, h, delay)) = gif_params(&frames, delay_ms)? else { return Ok(()) };
    let quantized = parallel_map_each(frames, move |img| quantize(img, w, h, delay));
    write_gif(path, w, h, quantized)
}

/// Compose `count` cells `cols` wide over a solid `background`, cell size taken
/// from the produced images, `gutter` px between/around. Reproduces both the
/// combined-GIF grid and the contact-sheet layout exactly (`cw = cell + gutter`,
/// canvas `gutter + cols*cw` by `gutter + rows*cw`).
fn compose_grid<F: FnMut(usize) -> RgbaImage>(
    count: usize,
    cols: u32,
    gutter: u32,
    background: [u8; 4],
    mut cell: F,
) -> RgbaImage {
    compose_cells(&(0..count).map(|i| cell(i)).collect::<Vec<_>>(), cols, gutter, background)
}

/// The layout half of [`compose_grid`], split out so callers that rendered
/// their cells on the pool can lay out the finished `cells` directly.
fn compose_cells(cells: &[RgbaImage], cols: u32, gutter: u32, background: [u8; 4]) -> RgbaImage {
    let count = cells.len();
    let cw = cells[0].width() + gutter;
    let rows = (count as u32 + cols - 1) / cols;
    let mut grid = RgbaImage::new(gutter + cols * cw, gutter + rows * cw);
    for px in grid.pixels_mut() {
        *px = Rgba(background);
    }
    for (i, c) in cells.iter().enumerate() {
        let x = gutter + (i as u32 % cols) * cw;
        let y = gutter + (i as u32 / cols) * cw;
        imageops::overlay(&mut grid, c, x as i64, y as i64);
    }
    grid
}

// ---------------------------------------------------------------------------
// Family A: spinning bodies
// ---------------------------------------------------------------------------

/// One spinning-body GIF. Sweeps `angle = TAU * f / frames` internally, upscales
/// each frame by `upscale` (nearest), encodes infinite-repeat at `delay_ms`.
///
/// Frames render on the pool ([`parallel_map`]) and are encoded in sweep order.
pub fn write_spin_gif<F>(path: &str, frames: usize, delay_ms: u32, upscale: u32, render: F) -> Res
where
    F: Fn(f32) -> RgbaImage + Send + Sync + 'static,
{
    let imgs = parallel_map(frames, move |f| {
        let angle = TAU * (f as f32) / (frames as f32);
        upscale_nearest(&render(angle), upscale)
    });
    encode_gif_parallel(path, delay_ms, imgs)
}

/// The "all types spinning together" grid GIF. `count` cells `cols` wide over
/// `background`, whole grid upscaled by `grid_upscale` per frame.
///
/// The pool splits this by *frame*, so each job owns a whole grid — its cells,
/// its compositing and its upscale — rather than the cells being a second fan-
/// out that [`parallel_map`] would collapse anyway.
pub fn write_spin_grid_gif<F>(
    path: &str,
    count: usize,
    cols: u32,
    frames: usize,
    delay_ms: u32,
    gutter: u32,
    background: [u8; 4],
    grid_upscale: u32,
    render: F,
) -> Res
where
    F: Fn(usize, f32) -> RgbaImage + Send + Sync + 'static,
{
    let imgs = parallel_map(frames, move |f| {
        let angle = TAU * (f as f32) / (frames as f32);
        let grid = compose_grid(count, cols, gutter, background, |i| render(i, angle));
        upscale_nearest(&grid, grid_upscale)
    });
    encode_gif_parallel(path, delay_ms, imgs)
}

/// The contact-sheet PNG: a `rows` × `cols` grid of stills, upscaled + saved.
/// `cell(row, col)` produces each cell.
///
/// Split by cell — there is only one image to encode, so unlike the GIF helpers
/// nothing downstream is serial enough to be worth grouping cells around.
pub fn write_contact_sheet<F>(
    path: &str,
    rows: u32,
    cols: u32,
    gutter: u32,
    background: [u8; 4],
    upscale: u32,
    cell: F,
) -> Res
where
    F: Fn(u32, u32) -> RgbaImage + Send + Sync + 'static,
{
    let count = (rows * cols) as usize;
    let cells = parallel_map(count, move |i| cell(i as u32 / cols, i as u32 % cols));
    let grid = compose_cells(&cells, cols, gutter, background);
    upscale_nearest(&grid, upscale).save(path)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Family B: scene / camera
// ---------------------------------------------------------------------------

/// Pure fit-to-viewport zoom. Reproduces each crate's `fit_zoom`:
/// solar `(0.92, 0.55)`, asteroid `(0.92, 0.50)`, comet `(0.90, 0.60)`,
/// moon `(0.92, 0.60)`.
pub fn fit_zoom(ext: f32, w: u32, h: u32, margin: f32, vsquash: f32) -> f32 {
    let halfw = w as f32 * 0.5 * margin;
    let halfh = h as f32 * 0.5 * margin;
    (halfw / ext).min(halfh / (ext * vsquash))
}

/// Orbit/animation GIF. Sweeps `t = span * f / frames` internally (same
/// associativity as the originals) and wraps each returned buffer as an image.
/// The closure gets `(w, h, t)` and returns a fresh `w*h*4` RGBA buffer.
pub fn write_orbit_gif<F: FnMut(u32, u32, f32) -> Vec<u8>>(
    path: &str,
    w: u32,
    h: u32,
    frames: usize,
    span: f32,
    delay_ms: u32,
    mut render: F,
) -> Res {
    let mut imgs = Vec::with_capacity(frames);
    for f in 0..frames {
        let t = span * f as f32 / frames as f32;
        let buf = render(w, h, t);
        imgs.push(RgbaImage::from_raw(w, h, buf).expect("buffer size matches"));
    }
    encode_gif(path, delay_ms, imgs)
}

/// General GIF driver for bespoke per-frame motion (the pan GIFs). The closure
/// gets `(f, frames, w, h)` so it can compute its own easing with the original
/// float expressions untouched, and returns a fresh `w*h*4` RGBA buffer.
pub fn write_anim_gif<F: FnMut(usize, usize, u32, u32) -> Vec<u8>>(
    path: &str,
    w: u32,
    h: u32,
    frames: usize,
    delay_ms: u32,
    mut render: F,
) -> Res {
    let mut imgs = Vec::with_capacity(frames);
    for f in 0..frames {
        let buf = render(f, frames, w, h);
        imgs.push(RgbaImage::from_raw(w, h, buf).expect("buffer size matches"));
    }
    encode_gif(path, delay_ms, imgs)
}

/// Single still PNG at a fixed `t`. Scene reporting stays in the caller.
pub fn write_poster<F: FnOnce(u32, u32, f32) -> Vec<u8>>(path: &str, w: u32, h: u32, t: f32, render: F) -> Res {
    let buf = render(w, h, t);
    RgbaImage::from_raw(w, h, buf).expect("buffer size matches").save(path)?;
    Ok(())
}
