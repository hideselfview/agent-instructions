// What differs between a board now and the board proposed. Each side is the
// list of elements its page drew (board.html's `drawnElements`): text, role,
// box (x, y, w, h in board pixels), text colour and background. `changesBetween`
// pairs the elements of the two lists and returns, for each side, a mark for
// every element that changed: its kind (missing, new, moved or restyled), its
// box, and a title that says what changed.
var CHANGE_MOVE = 2;

function boxDistance(a, b) {
  return Math.abs(a.x - b.x) + Math.abs(a.y - b.y) + Math.abs(a.w - b.w) + Math.abs(a.h - b.h);
}

// The part of the smaller box that the other box covers, from 0 to 1.
function boxOverlap(a, b) {
  const w = Math.min(a.x + a.w, b.x + b.w) - Math.max(a.x, b.x);
  const h = Math.min(a.y + a.h, b.y + b.h) - Math.max(a.y, b.y);
  if (w <= 0 || h <= 0) return 0;
  return (w * h) / Math.min(a.w * a.h, b.w * b.h);
}

function describe(e) {
  return e.text ? e.text : "a " + e.role;
}

function plural(n, word) {
  return n + " px " + word;
}

// Pairs of [now index, proposed index]: first the elements with the same text
// and role, the nearest first; then, of the elements left, those in the same
// place that cover most of each other, which are one element whose words or
// kind changed.
function pairElements(now, proposed) {
  const pairs = [];
  const usedNow = new Set(), usedProposed = new Set();
  const take = candidates => {
    candidates.sort((a, b) => a.rank - b.rank);
    for (const c of candidates) {
      if (usedNow.has(c.n) || usedProposed.has(c.p)) continue;
      usedNow.add(c.n);
      usedProposed.add(c.p);
      pairs.push([c.n, c.p]);
    }
  };
  const same = [];
  now.forEach((n, i) => proposed.forEach((p, j) => {
    if (n.role === p.role && n.text === p.text) same.push({n: i, p: j, rank: boxDistance(n, p)});
  }));
  take(same);
  const place = [];
  now.forEach((n, i) => proposed.forEach((p, j) => {
    if (usedNow.has(i) || usedProposed.has(j)) return;
    const covered = boxOverlap(n, p);
    if (covered >= 0.5) place.push({n: i, p: j, rank: 1 - covered});
  }));
  take(place);
  return pairs;
}

// The differences of one pair, as {kind, now, proposed}: the title of the
// circle on the now side (what the element becomes) and of the one on the
// proposed side (what it was); null when the element is as it was.
function differences(n, p) {
  const kinds = [], was = [], is = [];
  if (n.text !== p.text) {
    kinds.push("restyled");
    was.push("Was: " + describe(n));
    is.push("Becomes: " + describe(p));
  } else if (n.role !== p.role) {
    kinds.push("restyled");
    was.push("Was a " + n.role);
    is.push("Becomes a " + p.role);
  }
  if (n.color !== p.color) {
    kinds.push("restyled");
    was.push("Text colour was " + n.color);
    is.push("Text colour becomes " + p.color);
  }
  if (n.background !== p.background) {
    kinds.push("restyled");
    was.push("Background was " + (n.background || "none"));
    is.push("Background becomes " + (p.background || "none"));
  }
  const dx = p.x - n.x, dy = p.y - n.y, dw = p.w - n.w, dh = p.h - n.h;
  const moves = [];
  if (Math.abs(dx) > CHANGE_MOVE) moves.push(plural(Math.abs(dx), dx > 0 ? "right" : "left"));
  if (Math.abs(dy) > CHANGE_MOVE) moves.push(plural(Math.abs(dy), dy > 0 ? "down" : "up"));
  if (Math.abs(dw) > CHANGE_MOVE) moves.push(plural(Math.abs(dw), dw > 0 ? "wider" : "narrower"));
  if (Math.abs(dh) > CHANGE_MOVE) moves.push(plural(Math.abs(dh), dh > 0 ? "taller" : "shorter"));
  if (moves.length) {
    kinds.push("moved");
    was.push("Moved " + moves.join(", "));
    is.push("Moves " + moves.join(", "));
  }
  if (!kinds.length) return null;
  return {kind: kinds.includes("restyled") ? "restyled" : "moved", now: is.join("; "), proposed: was.join("; ")};
}

function changesBetween(now, proposed) {
  const marks = {now: [], proposed: []};
  const pairs = pairElements(now, proposed);
  const pairedNow = new Set(pairs.map(p => p[0])), pairedProposed = new Set(pairs.map(p => p[1]));
  const box = e => ({x: e.x, y: e.y, w: e.w, h: e.h});
  now.forEach((e, i) => {
    if (!pairedNow.has(i)) marks.now.push({kind: "missing", box: box(e), title: "Was: " + describe(e) + " (not in the proposal)"});
  });
  proposed.forEach((e, j) => {
    if (!pairedProposed.has(j)) marks.proposed.push({kind: "new", box: box(e), title: "New: " + describe(e)});
  });
  for (const [i, j] of pairs) {
    const d = differences(now[i], proposed[j]);
    if (!d) continue;
    marks.now.push({kind: d.kind, box: box(now[i]), title: d.now});
    marks.proposed.push({kind: d.kind, box: box(proposed[j]), title: d.proposed});
  }
  return marks;
}
