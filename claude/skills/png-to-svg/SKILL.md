---
name: png-to-svg
description: Redraw a flat, outlined PNG (a logo, icon, mascot or sticker) as a clean SVG the way an artist would, with one stroked centre line per outline stacked back to front instead of traced blobs. Use when asked to vectorize, trace or convert a PNG logo or icon to SVG, or to make an SVG that matches a PNG. Not for photographs or painted artwork.
---

# PNG to SVG

Draw the picture's structure; don't trace its pixels. A flat drawing is a stack
of shapes with outlines, so the SVG should be that stack. An auto-tracer gives
every outline two edges and every colour its own blob, which is why its output
is dimpled, heavy and impossible to edit.

Don't start with a tracer, not even as a first pass: a repaired trace needs many
more fixes, and ends with about twice the curves and file size of a drawing
built from the enclosed areas.

## When this applies

This fits artwork with solid colours and dark outlines. Check `cells.png` from
step 1: if the outlines split the picture into a few dozen clean areas, carry
on. For a photograph, or for soft painting with no outlines, say that a vector
version would lose what makes it look right, and ask what the user wants.

## Tools

`magick`, `rsvg-convert` and `xmllint`, plus Python with numpy, scipy and
scikit-image. Make a venv in the scratchpad for those. Work in a scratch folder,
because the scripts read and write the current folder. The commands below write
`$skill` for this skill's folder (`~/.claude/skills/png-to-svg`).

Crop the PNG to its solid pixels first. Keep the crop's size as the `viewBox`.

## Method

### 1. Find the areas

```sh
python "$skill/scripts/cells.py" source.png
```

It prints each enclosed area (label, box, mean colour) and the outline's width,
and writes `cells.png`. Look at `cells.png`: it shows the whole drawing's
structure. Name each area ("body", "face", "left ear").

### 2. Write the stack

List the shapes back to front, as a painter would: what's behind what. A shape
is one or more areas plus an outline width. Anything a later shape covers
doesn't have to be accurate, so close hidden edges with a plain polygon
(`poly`). For example, a screen partly covered by the hands that hold it is a
rounded rectangle behind them: take `convex_hull_image` of its area.

### 3. Draw each outline as one centre line

Grow the area by half the outline's width and fit its edge, in Python run with
`PYTHONPATH="$skill/scripts"` from the folder `cells.py` wrote into:

```python
from vectorlib import *
w = width(cells(241))
d, curves = closed(ring(grow(cells(241), w / 2)), tol=1.2, sig=2.5, deg=50)
```

- `tol` is the most a curve may stray, in pixels. 1 to 1.5 suits outlines.
- `sig` smooths along the line, which takes out dimples. Keep it below the size
  of the smallest bump you want to keep.
- `deg` is the turn that counts as a real corner. Corners stay sharp and the
  smoothing stops at them, so sharp points and dips keep their shape.

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
python "$skill/scripts/compare.py" source.png drawing.svg
```

Look at `sbs.png`, then all nine `tiles/`, then `heat.png`. Fix the places the
lists of missing and extra ink name, biggest first. The percentage is for
comparing rounds of one drawing, not a target: gradients and the source's soft
edges keep it above zero, and it can't see whether something looks wrong.

Before showing anyone the file:

- Render it in a browser engine as well as librsvg.
  - On macOS, run `swiftc -O "$skill/scripts/webkit_render.swift" -o web`, then
    `./web "$PWD/drawing.svg" "$PWD/out.png" WIDTH HEIGHT`.
  - Don't use headless Brave: it hangs on a file URL.
- Compare on the dark and light backgrounds it will sit on, and at 512, 128, 64
  and 32 pixels.
- Show the actual `.svg` in a viewer the user uses, not only your own PNG
  render. Label each side of any comparison.

## Known problems

- **Repeated ids.** When a group and the shape it draws share an id, a `<use>`
  refers to itself.
  - librsvg draws the shape and Chromium drops it, so the side-by-side looks
    right while Chromium-based viewers, such as VS Code, show nothing.
  - `compare.py` refuses repeated ids; name shapes `x-shape`.
- **Group stroke inheritance.** Inside a stroked group every clipped blob needs
  `stroke="none"`, or it draws a hairline stroke.
- **ImageMagick's SVG renderer.** It gives wrong comparisons. Use `rsvg-convert`
  directly.
- **Corners versus dimples.** `corners()` checks a line's turn close up and from
  further back.
  - A corner turns sharply close up and stays turned from further back.
  - A round tip turns only from further back, and a dimple only close up.
- **Thin white edge in the source.** Generated art often has a white line around
  its outlines. Leave it out unless the user wants it, and say so.
- **Gradients.** Flat colours lose soft glows. Say which ones went.

## Using the SVG in apps

Keep the source drawing as the one thing people edit.

- **macOS 26** shrinks a transparent PNG icon onto a gray tile, so make an
  `AppIcon.icon` folder with `icon.json` and `Assets/`.
  - Put the drawing on the 1024 canvas at about 840 wide.
  - Set `"glass": false` on the layer so colours stay true.
  - Check it with `xcrun actool`, which also makes the `.icns` that older macOS
    reads.
- **Android** needs a VectorDrawable for an adaptive icon.
  - Expand every `<use>`, and turn circles and ellipses into paths.
  - Turn each `clip-path` into a `<group>` with a `<clip-path>` child.
  - Scale the drawing so its furthest pixel falls inside the central 66dp circle
    of the 108dp canvas.

## Reporting

Give the curve count, the file size, and what still differs from the source.
Name the renderers you checked and the ones you didn't. Don't say it matches on
the strength of one render.
