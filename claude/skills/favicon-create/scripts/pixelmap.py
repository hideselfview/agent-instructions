#!/usr/bin/env python3
"""Work with a favicon drawn as a character map.

A map file lists its colours, a blank line, then the grid, one character per
pixel. "." is transparent:

    D #40183d
    C #fffcf4

    ..DD..
    .DCCD.

Usage:
  pixelmap.py svg MAP OUT.svg          write a crisp SVG, one rect per run
  pixelmap.py up MAP OUT_MAP           double a map with the Scale2x rule
  pixelmap.py sheet OUT.html NAME=MAP...   a labelled sheet of options
"""
import base64
import pathlib
import subprocess
import sys
import tempfile


def read(path):
    head, _, body = pathlib.Path(path).read_text().partition("\n\n")
    colours = {line.split()[0]: line.split()[1] for line in head.splitlines() if line.strip()}
    rows = [row for row in body.splitlines() if row]
    size = len(rows)
    if any(len(row) != size for row in rows):
        sys.exit(f"{path}: the grid must be square: {[len(row) for row in rows]} wide, {size} tall")
    unknown = {ch for row in rows for ch in row} - set(colours) - {"."}
    if unknown:
        sys.exit(f"{path}: no colour for {sorted(unknown)}")
    return colours, rows


def svg(colours, rows):
    size = len(rows)
    out = [
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {size} {size}" width="{size}" height="{size}" '
        'shape-rendering="crispEdges">\n'
    ]
    for y, row in enumerate(rows):
        x = 0
        while x < size:
            start, ch = x, row[x]
            while x < size and row[x] == ch:
                x += 1
            if ch != ".":
                out.append(f'<rect x="{start}" y="{y}" width="{x - start}" height="1" fill="{colours[ch]}"/>')
    return "".join(out) + "\n</svg>\n"


def scale2x(rows):
    size = len(rows)

    def at(y, x):
        return rows[y][x] if 0 <= y < size and 0 <= x < size else "."

    out = [["."] * (size * 2) for _ in range(size * 2)]
    for y in range(size):
        for x in range(size):
            p, a, b, c, d = at(y, x), at(y - 1, x), at(y, x + 1), at(y, x - 1), at(y + 1, x)
            e = [p, p, p, p]
            if c == a and c != d and a != b:
                e[0] = a
            if a == b and a != c and b != d:
                e[1] = b
            if d == c and d != b and c != a:
                e[2] = c
            if b == d and b != a and d != c:
                e[3] = d
            out[2 * y][2 * x], out[2 * y][2 * x + 1], out[2 * y + 1][2 * x], out[2 * y + 1][2 * x + 1] = e
    return ["".join(row) for row in out]


def png(colours, rows):
    with tempfile.TemporaryDirectory() as tmp:
        source, target = pathlib.Path(tmp, "a.svg"), pathlib.Path(tmp, "a.png")
        source.write_text(svg(colours, rows))
        subprocess.run(["rsvg-convert", str(source), "-o", str(target)], check=True)
        return base64.b64encode(target.read_bytes()).decode()


def sheet(out, options):
    sections = ""
    for option in options:
        name, path = option.split("=", 1)
        colours, rows = read(path)
        size, data = len(rows), png(colours, rows)
        uri = f"data:image/png;base64,{data}"

        def tab(background, text):
            return (
                f'<div class="tab" style="background:{background};color:{text}">'
                f'<img class="px" src="{uri}" width="16" height="16">Tab title</div>'
            )

        sections += (
            f'<section><h2>{name}</h2><div class="cells">'
            f'<figure><img class="px" style="background:#fff" src="{uri}" width="192" height="192">'
            f"<figcaption>{size}px grid, magnified, on white</figcaption></figure>"
            f'<figure><img class="px" style="background:#2b2c36" src="{uri}" width="192" height="192">'
            f"<figcaption>{size}px grid, magnified, on dark</figcaption></figure>"
            f'<figure>{tab("#fff", "#222")}{tab("#2b2c36", "#eee")}'
            "<figcaption>In a browser tab, actual size</figcaption></figure></div></section>"
        )
    pathlib.Path(out).write_text(
        "<!doctype html><meta charset=utf-8><style>"
        "body{margin:0;padding:24px;background:#f3f3f5;font:15px -apple-system,sans-serif;color:#222}"
        "h2{font-size:17px;margin:0 0 10px}section{margin-bottom:22px}"
        ".cells{display:flex;gap:32px;align-items:flex-start}figure{margin:0;width:200px}"
        "figcaption{margin-top:6px;color:#555;font-size:13px}.px{image-rendering:pixelated}"
        ".tab{display:flex;gap:8px;align-items:center;height:32px;padding:0 12px;"
        "border-radius:8px 8px 0 0;font-size:13px;margin-bottom:8px;width:150px;box-shadow:0 0 0 1px #ccc}"
        f"</style>{sections}"
    )


def main():
    command, *args = sys.argv[1:] or ["help"]
    if command == "svg":
        colours, rows = read(args[0])
        pathlib.Path(args[1]).write_text(svg(colours, rows))
    elif command == "up":
        colours, rows = read(args[0])
        palette = "".join(f"{key} {value}\n" for key, value in colours.items())
        pathlib.Path(args[1]).write_text(palette + "\n" + "\n".join(scale2x(rows)) + "\n")
    elif command == "sheet":
        sheet(args[0], args[1:])
    else:
        sys.exit(__doc__)


main()
