---
name: favicon-create
description: Draw a favicon or small icon by hand on the pixel grid, proving it at 16px and then upgrading it to 32px. Use when asked for a favicon, a tab icon, or a logo or mascot at 32px or under, or when a shrunk logo looks blurry or wrong at small sizes.
---

# Favicon

Draw it on the grid, one pixel at a time. Don't shrink the logo and don't draw a
small vector: 16 pixels can't hold a curve, so anything scaled down lands
between pixels and blurs.

On bae's sheep, shrinking the logo gave mush. Simplified vectors, over several
rounds, still looked wrong: the face wasn't sitting right on the grid. The first
hand-drawn map fixed that at once, and the second round had the pick.

## 1. Choose what survives

Look at the logo and name the few things that make it recognizable: its outer
shape, its one or two colours, one feature. Everything else goes. For the sheep
that was a round head, a pink headband and ear cups, and closed eyes. The body,
the player and the heart went.

Face the subject straight on, even if the logo is at an angle. A symmetrical
shape sits on a grid; an angled one doesn't.

## 2. The 16px proof

Write a character map, as `scripts/pixelmap.py` describes: colours, a blank
line, sixteen rows of sixteen characters. Take the colours from the logo.

- Outline one pixel wide, in the logo's darkest colour.

- Five to seven colours in all.

- A feature can be one pixel: a mouth, a cheek.

- A soft colour draws a soft edge: a shaded line can stand for a change of
  texture where a dark line would cut the shape in two.

- A mid-tone at the end of a line makes it read as a curve. The sheep's closed
  eye is four wide and two tall, mid-tones at its upper ends:

  ```
  M..M
  .DD.
  ```

Draw three options that differ in the feature that's hardest to get right, and
keep everything else still. Show the sheet and let the person choose:

```sh
python scripts/pixelmap.py sheet sheet.html "Option A=a.txt" "Option B=b.txt"
swiftc -O ../png-to-svg/scripts/webkit_render.swift -o web
./web "$PWD/sheet.html" "$PWD/sheet.png" 720 830
```

Every picture you show carries labels: which option, what size, on what
background. Keep an option's letter from round to round. An unlabelled strip of
sizes confused the person, and looking at it yourself first would have caught
that.

Read each option yourself before showing it, and redraw what's wrong:

- Eyes as a V read as check marks. Dots look startled. A flat line looks cross.
- A notch in the top of a head reads as cat ears.
- An outline fades on a dark tab; the fill has to carry the shape there.

## 3. The 32px upgrade

Only after the person picks. Sharp screens show a tab icon at 32 pixels, so it
needs its own drawing.

```sh
python scripts/pixelmap.py up f16.txt f32.txt
```

That doubles the map and rounds its corners with the Scale2x rule. Then redraw
what doubled badly by hand, which is usually the face: at 32 a curve can be a
real curve, so mid-tone tricks come out. Keep the proportions of the 16.

## 4. Deliver

```sh
python scripts/pixelmap.py svg f16.txt favicon-16.svg
python scripts/pixelmap.py svg f32.txt favicon-32.svg
```

Each size is its own drawing, so show it at exactly that size. Add a `<title>`
and `role="img"` with an `aria-label` to each SVG. For a site, also make the
`.ico` with `magick 16.png 32.png favicon.ico`. Keep the maps beside the SVGs:
they're what gets edited.

Say which renderers you checked, and that the favicons are separate drawings: a
change to the logo is redrawn here by hand.
