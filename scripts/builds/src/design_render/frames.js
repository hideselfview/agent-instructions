// A board's page in a frame, drawn at the board's own size (w by h) and
// scaled into a box: the frame stays live HTML, so a larger scale shows it
// sharp. The frame runs the page's scripts in a sandbox that cannot reach
// the page holding it. `boardFrame` makes the frame and its box, and
// `scaleFrame` scales the frame and sizes the box to match.
function boardFrame(src, w, h) {
  const frame = document.createElement("iframe");
  frame.setAttribute("sandbox", "allow-scripts");
  frame.setAttribute("title", src);
  frame.setAttribute("width", w);
  frame.setAttribute("height", h);
  frame.setAttribute("src", src);
  Object.assign(frame.style, {display: "block", border: "0", background: "transparent", transformOrigin: "0 0"});
  const box = document.createElement("div");
  box.className = "frame";
  box.append(frame);
  return {frame, box};
}
function scaleFrame(drawn, w, h, s) {
  drawn.frame.style.transform = "scale(" + s + ")";
  drawn.box.style.width = Math.round(w * s) + "px";
  drawn.box.style.height = Math.round(h * s) + "px";
}
