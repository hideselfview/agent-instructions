// Runs in a component's preview frame on the Designs page. The page sends
// the width it has room for ({previewWidth: N}) when the frame loads and
// when the preview says it is ready ({previewReady: true}). The script lays the
// preview's body out as wide as the widest thing in it that cannot wrap, at
// least NARROWEST, so that text wraps and a component that fills its row has
// room to fill as in a window, and at most the room. When the preview
// is then taller than TALL, the script sets the body's rows side by side in
// columns as wide as the one column was: the fewest columns that bring it
// under TALL, or as many as the room holds when none do. It sends the page
// the size the preview takes ({previewSize: {w, h}}) each time the size
// changes, so the page sizes the frame to fit the preview and never scales
// it.
// The review page asks for a whole preview in one column ({previewWidth: N,
// single: true}): the script never sets the rows in columns, and sends each
// row's height beside the size ({previewSize: {w, h}, rows: [{key, height}]},
// a row's key being its name, or its place). The review page puts the rows of
// the preview now beside the rows of the proposal, and answers each frame with
// the height every row takes in both ({previewAlign: [{key, height, spacer}]},
// a spacer standing for a row only the other frame has), which the script
// gives the rows as a least height, so that a row sits level with its twin.
(function () {
  const TALL = 640, NARROWEST = 480, GAP = 32;
  let room = null, sent = "", single = false, align = [];
  // Resolves once the preview has been laid out in the room the page gave it
  // with something in its body, which the review page's circles wait for.
  let laid;
  window.previewLaidOut = new Promise(resolve => { laid = resolve; });
  // The body's own inline style, and each row's, which a layout replaces.
  const own = {boxSizing: "", width: "", flexDirection: "", flexWrap: "", alignItems: "", columnGap: ""};
  const ownRow = new Map();
  // The least heights and spacers that line the rows up with the other frame's.
  const spacers = [], leveled = new Map();
  function clearAlign() {
    for (const spacer of spacers.splice(0)) spacer.remove();
    for (const [row, minHeight] of leveled) row.style.minHeight = minHeight;
    leveled.clear();
  }
  // Each row with its key: the text of its name (`.bae-preview-name`), else
  // its place, and a number after a key a row before it already has.
  function keyed(all) {
    const seen = new Map();
    return all.map((row, place) => {
      const name = row.querySelector(".bae-preview-name");
      const base = name && name.textContent.trim() ? name.textContent.trim() : "#" + place;
      const n = seen.get(base) || 0;
      seen.set(base, n + 1);
      return {key: n ? base + " " + (n + 1) : base, row};
    });
  }
  function applyAlign(all) {
    const rows = keyed(all);
    const at = key => rows.find(r => r.key === key);
    let next = 0;
    for (const entry of align) {
      const found = at(entry.key);
      if (entry.spacer || !found) {
        const spacer = document.createElement("div");
        spacer.style.height = entry.height + "px";
        spacer.style.flex = "none";
        const before = rows[next] ? rows[next].row : null;
        (before ? before.parentNode : document.body).insertBefore(spacer, before);
        spacers.push(spacer);
      } else {
        leveled.set(found.row, found.row.style.minHeight);
        found.row.style.minHeight = entry.height + "px";
        next = rows.indexOf(found) + 1;
      }
    }
  }
  function size() {
    const body = document.body, r = body.getBoundingClientRect(), m = getComputedStyle(body);
    return {
      w: Math.ceil(r.right + window.scrollX + parseFloat(m.marginRight)),
      h: Math.ceil(r.bottom + window.scrollY + parseFloat(m.marginBottom)),
    };
  }
  // The boxes the body lays out as its rows: its children, and the children
  // of a child that draws no box of its own (display: contents).
  function rows(of) {
    const found = [];
    for (const child of of.children) {
      const display = getComputedStyle(child).display;
      if (display === "contents") found.push(...rows(child));
      else if (display !== "none") found.push(child);
    }
    return found;
  }
  // Lays the rows out in one column, as wide as the widest thing in the
  // preview that cannot wrap, and at least NARROWEST.
  function column(body) {
    for (const [row, style] of ownRow) Object.assign(row.style, style);
    Object.assign(body.style, own, {boxSizing: "border-box", width: "min-content"});
    const least = Math.ceil(body.getBoundingClientRect().width);
    Object.assign(body.style, {width: Math.min(Math.max(least, NARROWEST), room) + "px"});
  }
  // Sets the body's rows in <n> columns, each <inner> wide, and returns the
  // size the body takes.
  function columns(body, all, n, inner, edges) {
    Object.assign(body.style, own, {boxSizing: "border-box", width: (edges + n * inner + (n - 1) * GAP) + "px",
      flexDirection: "row", flexWrap: "wrap", alignItems: "flex-start", columnGap: GAP + "px"});
    for (const row of all) {
      if (!ownRow.has(row)) ownRow.set(row, {width: row.style.width, flex: row.style.flex});
      Object.assign(row.style, {width: inner + "px", flex: "none"});
    }
    return size();
  }
  function lay() {
    const body = document.body;
    if (!body || room === null) return;
    clearAlign();
    column(body);
    let drawn = size();
    const all = rows(body);
    const natural = single ? keyed(all).map(({key, row}) => ({key, height: row.getBoundingClientRect().height})) : null;
    if (single) {
      applyAlign(all);
      drawn = size();
    } else if (drawn.h > TALL && all.length > 1) {
      const m = getComputedStyle(body);
      const edges = ["paddingLeft", "paddingRight", "borderLeftWidth", "borderRightWidth"]
        .reduce((sum, k) => sum + parseFloat(m[k]), 0);
      const inner = body.getBoundingClientRect().width - edges;
      let best = null, bestN = 0;
      for (let n = 2; n <= all.length && edges + n * inner + (n - 1) * GAP <= room; n++) {
        const set = columns(body, all, n, inner, edges);
        if (!best || set.h < best.h) { best = set; bestN = n; }
        if (set.h <= TALL) break;
      }
      if (best && best.h < drawn.h) drawn = columns(body, all, bestN, inner, edges);
      else column(body);
    }
    if (all.length) laid();
    const now = drawn.w + "x" + drawn.h + (natural ? JSON.stringify(natural) : "");
    if (now !== sent) {
      sent = now;
      parent.postMessage(natural ? {previewSize: drawn, rows: natural} : {previewSize: drawn}, "*");
    }
  }
  addEventListener("message", e => {
    if (e.source !== parent || !e.data) return;
    if (Array.isArray(e.data.previewAlign)) {
      align = e.data.previewAlign;
      lay();
      return;
    }
    if (typeof e.data.previewWidth !== "number") return;
    room = e.data.previewWidth;
    single = e.data.single === true;
    sent = "";
    lay();
  });
  addEventListener("DOMContentLoaded", () => {
    for (const k of Object.keys(own)) own[k] = document.body.style[k];
    new ResizeObserver(lay).observe(document.body);
    parent.postMessage({previewReady: true}, "*");
  });
})();
