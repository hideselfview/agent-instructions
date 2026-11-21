// Runs in a frame of the Designs page's Colors section (color_group.rs).
// It first mounts each component a scene names (data-component, a global
// name such as Bae.StatusGlyph, with its props as JSON in data-props), and
// an element names a component that the page lacks shows that problem in
// its place. Then, for each element of a group's scene that names a color
// (data-color), the script puts a chip in the column at the left of the
// scene: the color's name and a small swatch with one part per theme. It
// draws a line from the chip to the element: to the element's left edge, or
// under a word when the element says data-point="below". The chips keep the
// order of their elements from the top, each at its element's height or
// under the chip above it. The script tells the page the height the frame
// takes ({previewSize: {w, h}}) each time it changes. It asks the page for
// the switches' choices ({colorsReady: true}), and the page answers with the
// CSS of the options they pick and the theme the scenes draw in
// ({colorSwitch: {css, theme}}): the script puts the CSS last on the page and
// redraws every scene in that theme.
(function () {
  const GAP = 22, SVG = "http://www.w3.org/2000/svg";
  const body = document.body;
  const themes = body.dataset.themes.split(" ");
  const ground = body.dataset.ground;
  const use = document.querySelector(".cg-use");
  for (const element of document.querySelectorAll("[data-component]")) {
    const path = element.dataset.component;
    const component = path.split(".").reduce((at, key) => at && at[key], window);
    let problem = typeof component === "function" ? null : path + " is no component of the design system";
    if (!problem) {
      try {
        const props = element.dataset.props ? JSON.parse(element.dataset.props) : {};
        ReactDOM.flushSync(() => ReactDOM.createRoot(element).render(React.createElement(component, props)));
      } catch (error) {
        problem = path + " could not draw: " + error.message;
      }
    }
    if (problem) {
      element.className = "cg-problem";
      element.textContent = problem;
    }
  }
  const chips = [];
  if (use) {
    const gutter = use.querySelector(".cg-gutter");
    for (const element of use.querySelector(".cg-scene").querySelectorAll("[data-color]")) {
      const name = element.dataset.color;
      const chip = document.createElement("div");
      chip.className = "cg-chip";
      const label = document.createElement("span");
      label.className = "cg-nm";
      label.textContent = name;
      const mini = document.createElement("span");
      mini.className = "cg-mini";
      for (const theme of themes) {
        const part = document.createElement("span");
        part.dataset.theme = theme;
        part.style.background = `linear-gradient(var(--${name}), var(--${name})), var(--${ground})`;
        mini.append(part);
      }
      chip.append(label, mini);
      gutter.append(chip);
      chips.push({chip, element});
    }
  }
  const lines = document.createElementNS(SVG, "svg");
  lines.setAttribute("class", "cg-lines");
  if (use) use.append(lines);

  // Where a chip's line ends on <element>, in the coordinates of .cg-use.
  function point(element, origin) {
    const r = element.getBoundingClientRect();
    if (element.dataset.point === "below")
      return {x: r.left - origin.left + Math.min(8, r.width / 2), y: r.bottom - origin.top + 3};
    return {x: r.left - origin.left + Math.min(6, r.width / 2), y: r.top - origin.top + Math.min(r.height / 2, 10)};
  }

  function layout() {
    if (!use || !chips.length) return;
    const gutter = use.querySelector(".cg-gutter");
    const widest = Math.max(...chips.map(c => c.chip.offsetWidth));
    gutter.style.width = (widest + 20) + "px";
    use.style.minHeight = "";
    const origin = use.getBoundingClientRect();
    const placed = chips
      .map(c => ({...c, at: point(c.element, origin)}))
      .sort((a, b) => a.at.y - b.at.y);
    let last = -Infinity;
    const start = gutter.offsetWidth - 8;
    lines.replaceChildren();
    for (const c of placed) {
      const y = Math.max(c.at.y, last + GAP);
      last = y;
      c.chip.style.top = (y - 9) + "px";
      const line = document.createElementNS(SVG, "line");
      line.setAttribute("x1", start);
      line.setAttribute("y1", y);
      line.setAttribute("x2", c.at.x);
      line.setAttribute("y2", c.at.y);
      const dot = document.createElementNS(SVG, "circle");
      dot.setAttribute("cx", c.at.x);
      dot.setAttribute("cy", c.at.y);
      dot.setAttribute("r", 3);
      lines.append(line, dot);
    }
    if (last + 12 > origin.height) use.style.minHeight = Math.ceil(last + 12) + "px";
    lines.setAttribute("width", use.offsetWidth);
    lines.setAttribute("height", use.offsetHeight);
  }

  const picked = document.createElement("style");
  body.append(picked);
  window.addEventListener("message", e => {
    const choice = e.source === parent && e.data && e.data.colorSwitch;
    if (!choice) return;
    picked.textContent = choice.css;
    for (const scene of document.querySelectorAll(".cg-scene")) scene.dataset.theme = choice.theme;
    redraw();
  });
  parent.postMessage({colorsReady: true}, "*");

  let sent = "";
  function tell() {
    const size = {w: Math.ceil(document.documentElement.scrollWidth), h: Math.ceil(document.documentElement.scrollHeight)};
    const key = size.w + "x" + size.h;
    if (key === sent) return;
    sent = key;
    parent.postMessage({previewSize: size}, "*");
  }
  function redraw() {
    layout();
    tell();
  }
  redraw();
  document.fonts.ready.then(redraw);
  new ResizeObserver(redraw).observe(body);
})();
