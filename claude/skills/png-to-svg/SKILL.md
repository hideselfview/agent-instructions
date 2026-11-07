---
name: png-to-svg
description: Redraw a flat, outlined PNG (a logo, icon, mascot or sticker) as a clean SVG the way an artist would, with one stroked centre line per outline stacked back to front instead of traced blobs. Use when asked to vectorize, trace or convert a PNG logo or icon to SVG, or to make an SVG that matches a PNG. Not for photographs or painted artwork.
---

# PNG to SVG

Draw the picture's structure; don't trace its pixels. A flat drawing is a stack
of shapes with outlines, so the SVG should be that stack. An auto-tracer gives
every outline two edges and every colour its own blob, which is why its output
is dimpled, heavy and impossible to edit.

Don't start with a tracer, not even as a first pass. Tracing the sheep logo and
repairing it took a long list of fixes and ended at 652 curves and 37 KB.
Working from the enclosed areas took a handful of fixes and ended at 361 curves
and 17 KB. The tracing did teach what to look for, and that is written down
below.

## When this applies

This fits artwork with solid colours and dark outlines. Check `cells.png` from
step 1: if the outlines split the picture into a few dozen clean areas, carry
on. For a photograph, or for soft painting with no outlines, say that a vector
version would lose what makes it look right, and ask what the person wants.

## Tools

`magick`, `rsvg-convert` and `xmllint`, plus Python with numpy, scipy and
scikit-image. Make a venv in the scratchpad for those. Work in a scratch folder,
because the scripts read and write the current folder.

Crop the PNG to its solid pixels first. Keep the crop's size as the `viewBox`.

## Method

### 1. Find the areas

```sh
python scripts/cells.py source.png
```

It prints each enclosed area (label, box, mean colour) and the outline's width,
and writes `cells.png`. Read `cells.png`: this is the whole drawing's structure.
Name each area ("body", "face", "left ear").

### 2. Write the stack

List the shapes back to front, as a painter would: what's behind what. A shape
is one or more areas plus an outline width. Anything a later shape covers
doesn't have to be accurate, so close hidden edges with a plain polygon
(`poly`). A screen pinched by the hands that hold it is a rounded rectangle
behind the hands: take `convex_hull_image` of its area.

### 3. Draw each outline as one centre line

Grow the area by half the outline's width and fit its edge:

```python
from vectorlib import *
d, curves = closed(ring(grow(cells(241), width / 2)), tol=1.2, sig=2.5, deg=50)
```

- `tol` is the most a curve may stray, in pixels. 1 to 1.5 suits outlines.
- `sig` smooths along the line, which takes out dimples. Keep it below the size
  of the smallest bump you want to keep.
- `deg` is the turn that counts as a real corner. Corners stay sharp and the
  smoothing stops at them, so heart points and wool dips survive.

Use `stroke-linejoin="round"` and `stroke-linecap="round"`. Measure each shape's
width with `width(cells(...))`: real outlines vary, and one global width shows.

### 4. Shade with a clip

A shaded shape is three layers: the shape filled with the shadow colour, then
the light colour as a loose blob clipped to the shape, then the outline on top.
Only the blob's inner edge shows, so it can run on under the line and needs a
looser `tol` (about 2). Don't trace shading up to the outline.

```xml
<defs><path id="head-shape" d="..."/>
<clipPath id="in-head"><use xlink:href="#head-shape"/></clipPath></defs>
<g id="head">
  <use xlink:href="#head-shape" fill="#f6d9d2" stroke="none"/>
  <path clip-path="url(#in-head)" fill="#fffcf4" stroke="none" d="..."/>
  <use xlink:href="#head-shape" fill="none" stroke-width="11"/>
</g>
```

### 5. Measure the details by hand

Use real primitives where the art has them: `ellipse` for cheeks and discs,
`circle` for dots, and a heart in four curves. Measure from a zoomed crop rather
than guessing, and draw these directly. Placing whole-picture shapes by eye
never works; placing one eye or dot from measurements does.

### 6. Check, one square at a time

```sh
python scripts/compare.py source.png drawing.svg
```

Look at `sbs.png`, then all nine `tiles/`, then `heat.png`. Fix what the ink
lists point to, biggest first. The percentage is for comparing rounds of one
drawing, not a target: gradients and the source's soft edges keep it above zero,
and it can't see whether something looks wrong.

Before showing anyone the file:

- Render it in a browser engine as well as librsvg. On macOS:
  `swiftc -O scripts/webkit_render.swift -o web`, then
  `./web "$PWD/drawing.svg" "$PWD/out.png" WIDTH HEIGHT`. Headless Brave hung on
  a file URL, so don't wait on it.
- Compare on the dark and light backgrounds it will sit on, and at 512, 128, 64
  and 32 pixels.
- Show the actual `.svg` in a viewer the person uses, not only your own PNG
  render. Label each side of any comparison.

## Traps

- **Repeated ids.** A group and the shape it draws sharing an id makes a `<use>`
  point at itself. librsvg drew it and Chromium dropped the shape, so the wool
  vanished in VS Code while the side-by-side looked right. `compare.py` refuses
  repeated ids; name shapes `x-shape`.
- **Group stroke inheritance.** Inside a stroked group every clipped blob needs
  `stroke="none"`, or it grows a hairline.
- **ImageMagick's SVG renderer.** It gave false comparisons. Use `rsvg-convert`
  directly.
- **Corners versus dimples.** A corner turns sharply close up and stays turned
  from further back. A round tip turns only from further back, a dimple only
  close up. `corners()` checks both.
- **The source's thin white edge.** Generated art often has a white line around
  its outlines. Leave it out unless the person wants it, and say so.
- **Gradients.** Flat colours lose soft glows. Say which ones went.

## Using the SVG in apps

Keep the source drawing as the one thing people edit.

- **macOS 26:** a transparent PNG icon gets shrunk onto a gray tile. Make an
  `AppIcon.icon` folder with `icon.json` and `Assets/`, put the drawing on the
  1024 canvas at about 840 wide, set `"glass": false` on the layer so colours
  stay true, and check it with `xcrun actool`. It also makes the `.icns` older
  macOS reads.
- **Android:** an adaptive icon needs a VectorDrawable. Expand every `<use>`,
  turn circles and ellipses into paths, and turn `clip-path` into a `<group>`
  with a `<clip-path>` child. Scale the drawing so its furthest pixel sits
  inside the central 66dp circle of the 108dp canvas.

## Reporting

Give the curve count, the file size, and what still differs from the source.
Name the renderers you checked and the ones you didn't. Don't say it matches on
the strength of one render.
