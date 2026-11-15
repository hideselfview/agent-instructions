---
name: favicon-create
description: Draw a favicon or small icon by hand on the pixel grid, first at 16px and then redrawn at 32px. Use when asked for a favicon, a tab icon, or a logo or mascot at 32px or under, or when a shrunk logo looks blurry or wrong at small sizes.
---

# Favicon

Draw the icon on the grid, one pixel at a time. Don't shrink the logo and don't
draw a small vector: 16 pixels can't hold a curve, so a scaled-down drawing puts
its edges between pixels, and they blur. Simplified vectors still put features
off the grid; a hand-drawn map puts each one on it.

Work in a scratch folder. The commands below write `$skill` for this skill's
folder (`~/.claude/skills/favicon-create`).

## 1. Choose what to keep

Look at the logo and name the few things that make it recognizable: its outer
shape, its one or two colours and one feature. Everything else goes. For a
mascot, that might keep a round head, a coloured headband and closed eyes, and
leave out the body and anything it holds.

Face the subject straight on, even if the logo shows it at an angle. A
symmetrical shape fits a grid; an angled one doesn't.

## 2. Draw the 16px version

Write a character map, as `$skill/scripts/pixelmap.py` describes: colours, a
blank line, sixteen rows of sixteen characters. Take the colours from the logo.

- Draw the outline one pixel wide, in the logo's darkest colour.

- Use five to seven colours in all.

- A feature can be one pixel: a mouth, a cheek.

- A soft colour draws a soft edge: a shaded line can stand for a change of
  texture where a dark line would cut the shape in two.

- A mid-tone at the end of a line makes it look curved. This closed eye is four
  wide and two tall, with mid-tones at its upper ends:

  ```
  M..M
  .DD.
  ```

Draw three options that differ in the feature that's hardest to get right, and
keep everything else the same. Show the sheet and let the user choose:

```sh
python "$skill/scripts/pixelmap.py" sheet sheet.html "Option A=a.txt" "Option B=b.txt"
swiftc -O "$skill/../png-to-svg/scripts/webkit_render.swift" -o web
./web "$PWD/sheet.html" "$PWD/sheet.png" 720 830
```

Label every picture you show: which option, what size, on what background. Keep
an option's letter from round to round.

Look at each option yourself before you show it, and redraw what's wrong:

- Eyes drawn as a V look like check marks. Dots look startled. A flat line looks
  cross.
- A notch in the top of a head looks like cat ears.
- An outline fades on a dark tab, so the fill has to show the shape there.

## 3. Redraw at 32px

Start this only after the user picks an option. High-density screens show a tab
icon at 32 pixels, so it needs its own drawing.

```sh
python "$skill/scripts/pixelmap.py" up f16.txt f32.txt
```

That doubles the map and rounds its corners with the Scale2x rule. Then redraw
by hand what doubled badly, which is usually the face: at 32 a curve can be a
real curve, so take out the mid-tones that stood in for one. Keep the
proportions of the 16.

## 4. Deliver

```sh
python "$skill/scripts/pixelmap.py" svg f16.txt favicon-16.svg
python "$skill/scripts/pixelmap.py" svg f32.txt favicon-32.svg
```

Each size is its own drawing, so show it at exactly that size. Add a `<title>`
and `role="img"` with an `aria-label` to each SVG. For a site, also make the
`.ico`: render each SVG to a PNG at its size
(`rsvg-convert favicon-16.svg -o 16.png`), then run
`magick 16.png 32.png favicon.ico`. Keep the maps beside the SVGs, and edit the
maps, not the SVGs.

Say which renderers you checked, and that the favicons are separate drawings: a
change to the logo has to be redrawn here by hand.
