#!/usr/bin/env python3
"""Build components/bundle.js from components/src/*.js, and components/bundle.css from components/src/*.css.

The design system's format takes one classic script, components/bundle.js. Its
sources are in components/src/, read in file-name order; the last
(99-export.js) registers every component on window.Builds. Line 1 of the
result is the header the format reads: the namespace and the components. The
style sheet's parts are joined in file-name order, which is the order of the
cascade.

Usage: build-bundle.py [components-dir]   (default: components beside this file)
"""
import json
import pathlib
import re
import sys

components = pathlib.Path(sys.argv[1]) if len(sys.argv) > 1 else pathlib.Path(__file__).with_name("components")
sources = sorted((components / "src").glob("*.js"))
if not sources:
    sys.exit(f"no sources in {components / 'src'}")
body = "".join(path.read_text() for path in sources)
exports = body[body.rindex("window.Builds = Object.assign(window.Builds || {}, {"):]
names = re.findall(r"(\w+): \1\b", exports)
header = {"format": 4, "namespace": "Builds", "components": [{"name": name} for name in names]}
(components / "bundle.js").write_text("/* @ds-bundle: " + json.dumps(header, separators=(",", ":")) + " */\n" + body)
print(f"bundle.js: {len(sources)} sources, {len(names)} components")
styles = sorted((components / "src").glob("*.css"))
if not styles:
    sys.exit(f"no style sheets in {components / 'src'}")
(components / "bundle.css").write_text("".join(path.read_text() for path in styles))
print(f"bundle.css: {len(styles)} parts")
