# dither-core

Bayer ordered dithering and level quantization — the pixel-art output stage every
renderer ends with. 31 lines, no third-party dependencies.

## Why ordered dither

- Kills ramp banding on a sphere's shading gradient.
- Dithers the day/night terminator, which is otherwise a hard arc.
- Stays crisp under spin, where an error-diffusion dither would crawl.

`quant` snaps a channel to a fixed number of levels; the planet renderer's step is
22 levels (12/255), which is worth knowing when reading GPU/CPU pixel diffs — a
1e-7 difference before `quant` becomes a whole level after it.

## Anchor the dither to the thing, not the screen

A dither matrix indexed by *screen* position stipples differently every time the
camera moves, and the stipple crawls. `background-core`'s nebula therefore anchors
its dither to the cloud field's world position, which is also what lets the whole
backdrop layer scroll as a sprite instead of being rebuilt on a pan. See
[background-core](../background-core/README.md).

## GLSL

`src/dither.glsl` is the port. It rides along inside `noise-core`'s prelude rather
than being included separately, so every GPU shader in the workspace has it.
