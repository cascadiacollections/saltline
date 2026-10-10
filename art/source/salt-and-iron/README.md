# Salt & Iron game assets

Original source: [01-salt-and-iron.png](https://cascadiacollections.github.io/saltline/assets/concept-boards/01-salt-and-iron.png).
The unchanged board is kept here so the conversion never needs a network request.

The shipping chassis and structures are cut directly from that board, rather
than redrawn approximations. The light, heavy and mobile showcase chassis become
Base, Ballast and Forced March. The showcase's intact, damaged and collapsed
water towers become the three water-tower states; the damaged cutout is sheared
two native pixels at the crown to retain the game's leaning feedback. The main
scene's lattice towers supply Derrick, Radio Mast and Pylon; the tank body
supplies Silo. Their other damage states use the existing offline shear/rubble
passes. Seasonal costumes retain the generated artwork.

The title background uses the original board's complete hero scene, with its
baked-in lettering removed by interpolating adjacent source sky pixels. The game's own
title and menu continue to draw on top; the original firing gun, salt terrain
and foreground works belong only to that title illustration.
The wide hero scene keeps its original aspect ratio, anchors its terrain to the
bottom and extends the original sky above it to fill the title screen.
Title labels receive small opaque backings so the denser artwork does not
compete with menu text; menu positions and controls stay the same.

`tools/import_concept_art.py` records the source rectangles and chassis masks.
It removes showcase charcoal or foreground blue sky, samples original pixels
onto the native grid and maps to the existing palette. Olive armour maps to hull
roles; warm hatches and pale metal retain their accent roles. Chassis sampling
preserves the existing (15,3) and Ballast (13,4) muzzle anchors. Trestles narrow
to the water tower's existing nine-pixel footing and contact row 32. Every sprite
canvas is checked against `native-contracts.json` before writing.

Two cleanup images were produced with the built-in ImageGen tool on 2026-10-10:

- `clean-background.png`: the board's sky, clouds, industrial harbour and water,
  with text, foreground targets, terrain and the showcase removed. Conversion
  crops the added lower water to recover the original main-scene framing.
- `extracted-barrel.png`: the isolated barrel, aligned horizontally. Conversion
  uses nearest sampling to prepare nineteen 28x28 frames at 5-degree steps for
  each of the six existing build variants. There is no runtime rotation.

Both cleanup images are edited derivatives, not byte-exact original cutouts.
The chassis and structure images continue to use the original board itself.

The native harbour PNG is accompanied by `.pal8`: exactly 320x180 palette-slot
bytes, embedded in the Rust presentation crate. The PNG and embedded indices
are generated together. Its colours use the sky ramp plus pale rust/white
lights, reserving alert amber for gameplay markers and versus side ink.
The static backdrop paints these pixels one-for-one,
mirrors panels for wider buffers, extends navy above tall buffers, and maps sky
tones darker at dusk/night. Both flat and cached layered paths use this same
artwork. Existing stars and biome palettes remain cosmetic overlays. No sky
pixels enter terrain or collision queries.

Regenerate all shipping art and review sheets with:

```sh
uv run python tools/gen_sprites.py
uv run python -m unittest discover -s tools -p 'test_concept_art.py'
```

The conversion runs offline and leaves the simulation crate, seed stream,
heightfields, target boxes, aiming grid and solver unchanged. `concept_assets_preview_4x.png`
is the native fallback contact sheet. Runtime uses the source-color artwork
in `art/detail/` at four display pixels per logical unit; `art/detail/preview.png`
is the detailed contact sheet.

## Detailed presentation

`tools/build_detail_art.py` samples the original board directly into four-times
logical sprite canvases, preserving source RGB values instead of quantizing them.
The pivots remain (15,3), (13,4) and barrel (3,24) in logical units. Standing tower
feet remain at logical row 32 and the extra bottom row remains transparent.
Source-color damage states use the board's damaged tower and wreck fragments;
other leaning structures are sheared offline about their existing footing.
Native seasonal artwork remains the fallback for costumes.

Soil samples come from the board's salt roof, rock cross-section and deep soil.
Texture coordinates stay attached to world columns, and only existing heightfield
spans receive texture. The source-color harbour plate and soil tile bytes are
embedded in Rust; texture PNG images serve as review/reproducibility artifacts. Native
palette assets remain available for regression comparisons. Detailed frames use
the flat painter; all later UI, terrain and effect writes retain painter order.

The hero image keeps the original artwork outside the lettering area byte-for-byte
before resampling. Its cleanup does not paste a differently lit sky rectangle.

## Cleanup prompts

Background prompt (built-in ImageGen, input: the original board, opaque):

> Use case: precise-object-edit / background-extraction. Edit target: supplied Salt Line Salt & Iron board. Extract and clean the actual main scene's BACKGROUND ONLY into a seamless wide 16:9 game background plate. Preserve the board's actual navy dusk sky, stepped blue clouds, distant industrial harbour skyline, cranes, chimneys, few amber window lights, sun at far left and calm blue water reflections. Remove the SALT LINE lettering and subtitle, remove every foreground gun, muzzle flash, rust water tower, radio towers, foreground buildings and piping, white salt terrain and brown earth cross-section, and the entire bottom asset showcase panel. Fill removed areas consistently with the EXISTING adjacent distant blue water / sky / far skyline; no new foreground objects, no new rust structures, no salt hills. Distant skyline is at approximately 60 percent of image height; below it the lower 40 percent is simple restrained blue water. Upper quarter stays clean dark navy for gameplay HUD. Retain original pixel-art look and existing board details as faithfully as possible. Full image opaque. Do not redraw a stylistically different scene. No text, no labels, no UI, no palette swatches.

Barrel prompt (built-in ImageGen, input: the original board, transparent):

> Use case: background-extraction. Extract JUST the long olive articulated artillery gun barrel from the LIGHT tracked gun in the bottom-left gun showcase of the supplied Salt & Iron board. Preserve its actual olive and pale-metal rim texture, segmented tube, shadow underside and thicker muzzle collar. Remove gun chassis, turret, background, dirt and all lettering. Present one isolated straight barrel aligned HORIZONTALLY pointing to the RIGHT, with the rear mounting end on the left and muzzle on the right, on a genuinely transparent canvas; preserve side-view original detail and colours. Wide horizontal composition tightly around the barrel with transparent margin on all sides. No invented objects, no labels, no glow or shadows outside the object. Game will downsample it and prerender angled frames offline.
