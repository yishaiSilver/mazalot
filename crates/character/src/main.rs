//! Proof-of-concept sprite compositor.
//!
//! Demonstrates the same idea `rebels-in-the-sky` uses, from scratch and with
//! ZERO art assets on disk:
//!
//!   1. Base "parts" are tiny pixel grids, drawn here as ASCII text. A non-artist
//!      edits a part by typing characters in a 16x16 grid.
//!   2. Parts are drawn in placeholder marker colors: R = primary, G = highlight,
//!      B = shadow. A `ColorMap` recolors those markers into real skin / hair /
//!      jersey colors. One drawn part therefore yields unlimited looks.
//!   3. Characters are COMPOSED by layering parts (body -> shirt -> hair -> eyes),
//!      all chosen from a seeded RNG so a given seed always rebuilds the same
//!      character (exactly how the reference game reproduces sprites from a seed).
//!   4. Backgrounds are PURELY procedural (a code-generated starfield) -- the class
//!      of subject where "generate from math" actually looks good.
//!
//! Output: two upscaled PNG contact sheets in ./out/.

use image::{imageops, Rgba, RgbaImage};
use rand::{Rng, SeedableRng};
use rand_chacha::ChaCha8Rng;

const W: u32 = 16; // sprite width  (px)
const H: u32 = 16; // sprite height (px)

// ---------------------------------------------------------------------------
// Base parts. '.' transparent, R/G/B recolor markers, K black, W white.
// You "draw" by editing these grids. That's the entire art skill required.
// ---------------------------------------------------------------------------

// Body: head + torso + legs, drawn in skin markers (B on the right = shadow side).
const BODY: &str = "\
................
................
....RRRRRR......
...RRRRRRRB.....
...RRRRRRRB.....
...RRRRRRRB.....
...RRRRRRRB.....
....RRRRRB......
.....RRRB.......
...RRRRRRRR.....
..RRRRRRRRRB....
..RRRRRRRRRB....
..RRRRRRRRRB....
...RR....RR.....
...RR....RB.....
...RB....RB.....";

// Eyes: fixed black pixels on the face (no recoloring).
const EYES: &str = "\
................
................
................
................
................
.....K..K......
................
................
................
................
................
................
................
................
................
................";

const HAIR_STYLES: &[&str] = &[
    // short cap
    "\
................
....RRRRRR......
...RRRRRRRR.....
...R......R.....
................
................
................
................
................
................
................
................
................
................
................
................",
    // long, framing the face
    "\
................
....RRRRRR......
...RRRRRRRR.....
...RR....RR.....
...RR....RR.....
...RR....RR.....
...R......R.....
................
................
................
................
................
................
................
................
................",
    // spiky top
    "\
....R..R..R.....
....RRRRRR......
...RRRRRRRR.....
...R......R.....
................
................
................
................
................
................
................
................
................
................
................
................",
    // bald (empty) -- variety includes "no hair"
    "................",
];

const SHIRT_STYLES: &[&str] = &[
    // classic jersey (solid, shaded)
    "\
................
................
................
................
................
................
................
................
................
...RRRRRRRR.....
..RRRRRRRRRB....
..RRRRRRRRRB....
..RRRRRRRRRB....
................
................
................",
    // horizontal stripe (R primary / G accent)
    "\
................
................
................
................
................
................
................
................
................
...RRRRRRRR.....
..GGGGGGGGGG....
..RRRRRRRRRB....
..GGGGGGGGGB....
................
................
................",
    // vest (open front)
    "\
................
................
................
................
................
................
................
................
................
...RR....RR.....
..RRRB..GRRB....
..RRRB..GRRB....
..RRRB..GRRB....
................
................
................",
];

// ---------------------------------------------------------------------------
// Color maps: recolor the R/G/B markers into a real 3-tone palette.
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct Palette {
    primary: [u8; 3],
    highlight: [u8; 3],
    shadow: [u8; 3],
}

// A handful of skin tones -- realistic and alien. Add one = every character
// pool that uses it gains variety, for free.
const SKINS: &[Palette] = &[
    Palette { primary: [235, 190, 150], highlight: [250, 215, 180], shadow: [190, 140, 110] }, // light
    Palette { primary: [200, 150, 110], highlight: [225, 180, 140], shadow: [150, 100, 75] },  // tan
    Palette { primary: [140, 95, 65], highlight: [175, 125, 90], shadow: [95, 60, 40] },       // brown
    Palette { primary: [110, 190, 120], highlight: [150, 220, 150], shadow: [70, 140, 85] },   // green alien
    Palette { primary: [130, 150, 220], highlight: [165, 185, 245], shadow: [90, 105, 170] },  // blue alien
    Palette { primary: [190, 130, 200], highlight: [220, 165, 230], shadow: [140, 90, 150] },  // violet alien
];

const HAIRS: &[Palette] = &[
    Palette { primary: [40, 35, 35], highlight: [80, 72, 70], shadow: [20, 18, 18] },       // black
    Palette { primary: [110, 75, 45], highlight: [150, 110, 70], shadow: [75, 50, 30] },    // brown
    Palette { primary: [225, 200, 120], highlight: [245, 230, 170], shadow: [180, 150, 80] }, // blonde
    Palette { primary: [200, 70, 55], highlight: [235, 110, 90], shadow: [150, 45, 35] },   // red
    Palette { primary: [230, 235, 240], highlight: [255, 255, 255], shadow: [180, 185, 195] }, // white
    Palette { primary: [90, 130, 210], highlight: [130, 165, 235], shadow: [60, 95, 165] }, // dyed blue
];

const JERSEYS: &[Palette] = &[
    Palette { primary: [210, 60, 60], highlight: [240, 110, 110], shadow: [150, 40, 40] },   // red
    Palette { primary: [60, 110, 210], highlight: [110, 155, 240], shadow: [40, 75, 150] },  // blue
    Palette { primary: [230, 190, 60], highlight: [250, 220, 110], shadow: [175, 140, 40] }, // gold
    Palette { primary: [70, 175, 110], highlight: [115, 210, 150], shadow: [45, 125, 80] },  // green
    Palette { primary: [45, 45, 55], highlight: [90, 90, 105], shadow: [25, 25, 32] },       // charcoal
    Palette { primary: [200, 90, 175], highlight: [230, 130, 205], shadow: [150, 60, 130] }, // magenta
];

/// Turn an ASCII grid into a marker image (still in R/G/B placeholder colors).
fn part_image(template: &str) -> RgbaImage {
    let mut img = RgbaImage::new(W, H);
    for (y, line) in template.lines().enumerate() {
        if y as u32 >= H {
            break;
        }
        for (x, ch) in line.chars().enumerate() {
            if x as u32 >= W {
                break;
            }
            let px = match ch {
                'R' => Rgba([255, 0, 0, 255]),
                'G' => Rgba([0, 255, 0, 255]),
                'B' => Rgba([0, 0, 255, 255]),
                'K' => Rgba([0, 0, 0, 255]),
                'W' => Rgba([255, 255, 255, 255]),
                _ => Rgba([0, 0, 0, 0]), // '.' and anything else
            };
            img.put_pixel(x as u32, y as u32, px);
        }
    }
    img
}

/// Recolor the R/G/B markers of a part into a real palette, in place.
fn apply_palette(img: &mut RgbaImage, p: Palette) {
    for px in img.pixels_mut() {
        let [r, g, b, a] = px.0;
        if a == 0 {
            continue;
        }
        let mapped = match (r, g, b) {
            (255, 0, 0) => Some(p.primary),
            (0, 255, 0) => Some(p.highlight),
            (0, 0, 255) => Some(p.shadow),
            _ => None, // fixed colors (black eyes, white) pass through untouched
        };
        if let Some([nr, ng, nb]) = mapped {
            *px = Rgba([nr, ng, nb, 255]);
        }
    }
}

fn pick<'a, T>(rng: &mut ChaCha8Rng, items: &'a [T]) -> &'a T {
    &items[rng.gen_range(0..items.len())]
}

/// Build one fully-composed character from a seed. Same seed -> same character.
fn compose_character(seed: u64) -> RgbaImage {
    let mut rng = ChaCha8Rng::seed_from_u64(seed);

    let skin = *pick(&mut rng, SKINS);
    let hair = *pick(&mut rng, HAIRS);
    let jersey = *pick(&mut rng, JERSEYS);
    let hair_style = pick(&mut rng, HAIR_STYLES);
    let shirt_style = pick(&mut rng, SHIRT_STYLES);

    let mut canvas = RgbaImage::new(W, H);

    // Layer order matters: body first, then clothing/hair/eyes on top.
    let mut body = part_image(BODY);
    apply_palette(&mut body, skin);
    imageops::overlay(&mut canvas, &body, 0, 0);

    let mut shirt = part_image(shirt_style);
    apply_palette(&mut shirt, jersey);
    imageops::overlay(&mut canvas, &shirt, 0, 0);

    let mut hair_img = part_image(hair_style);
    apply_palette(&mut hair_img, hair);
    imageops::overlay(&mut canvas, &hair_img, 0, 0);

    // Eyes are fixed color -- no palette applied.
    let eyes = part_image(EYES);
    imageops::overlay(&mut canvas, &eyes, 0, 0);

    canvas
}

/// A purely procedural starfield -- no assets. This is the subject class where
/// "generate from math" genuinely looks good.
fn starfield(seed: u64) -> RgbaImage {
    let mut rng = ChaCha8Rng::seed_from_u64(seed ^ 0x5EED_5EED);
    let mut bg = RgbaImage::new(W, H);
    for px in bg.pixels_mut() {
        *px = Rgba([12, 10, 26, 255]); // deep space navy
    }
    for _ in 0..10 {
        let x = rng.gen_range(0..W);
        let y = rng.gen_range(0..H);
        let b = rng.gen_range(120u8..=255);
        bg.put_pixel(x, y, Rgba([b, b, b, 255]));
    }
    bg
}

/// Place a small sprite onto a background cell.
fn cell(seed: u64, with_bg: bool) -> RgbaImage {
    let mut base = if with_bg {
        starfield(seed)
    } else {
        let mut b = RgbaImage::new(W, H);
        for px in b.pixels_mut() {
            *px = Rgba([30, 30, 38, 255]);
        }
        b
    };
    let sprite = compose_character(seed);
    imageops::overlay(&mut base, &sprite, 0, 0);
    base
}

/// Assemble cells into an upscaled grid PNG.
fn contact_sheet(cells: &[RgbaImage], cols: u32, scale: u32, gutter: u32) -> RgbaImage {
    let rows = (cells.len() as u32 + cols - 1) / cols;
    let cw = W + gutter;
    let ch = H + gutter;
    let mut sheet = RgbaImage::new(gutter + cols * cw, gutter + rows * ch);
    for px in sheet.pixels_mut() {
        *px = Rgba([18, 18, 24, 255]);
    }
    for (i, c) in cells.iter().enumerate() {
        let cx = gutter + (i as u32 % cols) * cw;
        let cy = gutter + (i as u32 / cols) * ch;
        imageops::overlay(&mut sheet, c, cx as i64, cy as i64);
    }
    imageops::resize(
        &sheet,
        sheet.width() * scale,
        sheet.height() * scale,
        imageops::FilterType::Nearest,
    )
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    std::fs::create_dir_all("out")?;

    // Sheet 1: 48 fully-random characters over procedural starfields.
    // Variety here comes from parts * colors * layers, all from the seed.
    let roster: Vec<RgbaImage> = (0..48).map(|i| cell(1000 + i as u64, true)).collect();
    let sheet1 = contact_sheet(&roster, 8, 10, 2);
    sheet1.save("out/characters.png")?;

    // Sheet 2: isolate the RECOLOR multiplier. Same body+shirt+hair SHAPES,
    // only the palettes change -- one drawn part set, many identities.
    let recolor: Vec<RgbaImage> = (0..8)
        .map(|i| {
            // Force identical shapes by fixing the shape choices, varying only colors.
            let mut rng = ChaCha8Rng::seed_from_u64(7);
            let hair_style = pick(&mut rng, HAIR_STYLES);
            let shirt_style = pick(&mut rng, SHIRT_STYLES);

            let mut crng = ChaCha8Rng::seed_from_u64(500 + i as u64);
            let skin = *pick(&mut crng, SKINS);
            let hair = *pick(&mut crng, HAIRS);
            let jersey = *pick(&mut crng, JERSEYS);

            let mut canvas = RgbaImage::new(W, H);
            let mut body = part_image(BODY);
            apply_palette(&mut body, skin);
            imageops::overlay(&mut canvas, &body, 0, 0);
            let mut shirt = part_image(shirt_style);
            apply_palette(&mut shirt, jersey);
            imageops::overlay(&mut canvas, &shirt, 0, 0);
            let mut hair_img = part_image(hair_style);
            apply_palette(&mut hair_img, hair);
            imageops::overlay(&mut canvas, &hair_img, 0, 0);
            imageops::overlay(&mut canvas, &part_image(EYES), 0, 0);

            let mut bg = RgbaImage::new(W, H);
            for px in bg.pixels_mut() {
                *px = Rgba([30, 30, 38, 255]);
            }
            imageops::overlay(&mut bg, &canvas, 0, 0);
            bg
        })
        .collect();
    let sheet2 = contact_sheet(&recolor, 8, 14, 2);
    sheet2.save("out/recolor_demo.png")?;

    let combos = SKINS.len()
        * HAIRS.len()
        * JERSEYS.len()
        * HAIR_STYLES.len()
        * SHIRT_STYLES.len();
    println!("Wrote out/characters.png (48 random crew)");
    println!("Wrote out/recolor_demo.png (same shapes, 8 recolors)");
    println!(
        "Part library: {} bodies-worth of hand-authored pixels -> {} distinct combinations",
        1, combos
    );
    Ok(())
}
