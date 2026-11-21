This folder holds the builds dashboard's design system: the colors and fonts of
the page the builds daemon serves (scripts/builds/src/daemon/page.html), and its
stylesheet. The boards in `../mocks` use this stylesheet and these tokens.

## Using this system

`tokens.json` holds the colors, one value for Dark and one for Light, and the
two font stacks. `builds design tokens-css DIR` writes `tokens.css` from it.
`colors.json` puts each color in the group of its role, which the dashboard's
Design system tab draws as swatches under each group's line. `build-bundle.py`
builds `components/bundle.css` and `components/bundle.js` from
`components/src/`; edit the sources, not the bundles. `bundle.js` sets
`window.Builds`, which holds no components: the boards copy the dashboard's
markup and style it with the classes in `bundle.css`.
