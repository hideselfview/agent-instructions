The builds dashboard's design system: the colors and fonts of the page the
builds daemon serves (scripts/builds/src/daemon/page.html), and its stylesheet.
The boards in `../mocks` draw with it.

## Using this system

`tokens.json` holds the colors, one value for Dark and one for Light, and the
two font stacks. `builds design tokens-css DIR` writes `tokens.css` from it.
`build-bundle.py` builds `components/bundle.css` and `components/bundle.js` from
`components/src/`; edit the sources, not the bundles. `bundle.js` sets
`window.Builds`, which holds no components yet: the boards are the dashboard's
markup, drawn with the classes of `bundle.css`.
