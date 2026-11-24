// The Designs page's contents column: the list of the chosen tab's sections
// at the page's right, under the Filter field. An entry is {name, subs, ...}
// whose subs are entries too. The entry that the person has on screen is the
// current one: the column marks it and opens its group. The functions that
// draw use the page's `h`.

// The entries of a canvas page: its row headings (the server's `headings`,
// each {name, y} or, for a heading drawn once for each platform, {name, y,
// subs}), each with the row it stands for: the drawn <boards> from its y down
// to the next heading's y (the first row also holds the boards above it). A
// row with no drawn board has no entry, so the Filter field narrows the column
// along with the boards.
function boardEntries(headings, boards) {
  const rows = headings.flatMap(e => e.subs || [e]).sort((a, b) => a.y - b.y);
  const rowOf = leaf => {
    const n = rows.indexOf(leaf);
    const from = n ? leaf.y : -Infinity, to = n + 1 < rows.length ? rows[n + 1].y : Infinity;
    return {boards: boards.filter(b => b.y >= from && b.y < to)};
  };
  const entry = leaf => ({name: leaf.name, y: leaf.y, row: rowOf(leaf), subs: []});
  const kept = e => e.row.boards.length > 0;
  return headings.flatMap(e => {
    if (!e.subs) return [entry(e)].filter(kept);
    const subs = e.subs.map(entry).filter(kept);
    return subs.length ? [{name: e.name, y: e.y, row: subs[0].row, subs}] : [];
  });
}
// The entry of <entries> whose row holds the height <y> of the canvas: the
// last row that starts at or above it, or the first row when <y> is above them
// all. A platform pair gives its platform's entry. Null with no entry.
function rowAt(entries, y) {
  const leaves = entries.flatMap(e => e.subs.length ? e.subs : [e]).sort((a, b) => a.y - b.y);
  return leaves.filter(e => e.y <= y).pop() || leaves[0] || null;
}
// The left, top and width, on the canvas, of <row>'s drawn boards.
function rowBox(row) {
  const left = Math.min(...row.boards.map(b => b.x)), top = Math.min(...row.boards.map(b => b.y));
  return {left, top, width: Math.max(...row.boards.map(b => b.x + b.w)) - left};
}

// The entries of the design system tab under <main>: each section that shows
// (Colors with its groups, Type) and, in place of the Components section, each
// of its groups that shows with its subgroups. A part that the Filter field
// hides has no entry. An entry has the `section` that holds it and the
// `heading` that a click scrolls to.
function systemEntries(main) {
  const entry = (section, subs) => ({
    name: section.querySelector(":scope > h2, :scope > h3, :scope > h4").textContent,
    section,
    heading: section.querySelector(":scope > h2, :scope > h3, :scope > h4"),
    subs: subs.filter(s => !s.section.hidden),
  });
  const group = g => entry(g, [...g.querySelectorAll(":scope > .ds-sub")].map(s => entry(s, [])));
  const shown = parts => parts.filter(p => !p.hidden);
  return shown([...main.querySelectorAll(":scope > .ds-sec")]).flatMap(sec => sec.hasAttribute("data-lift")
    ? shown([...sec.querySelectorAll(":scope > .ds-groups > .ds-group")]).map(group)
    : [entry(sec, [...sec.querySelectorAll(":scope > .ds-group, :scope > .ds-groups > .ds-group")].map(g => entry(g, [])))]);
}
// The top, in the window, that <entry>'s heading sticks at, or <bar> (the
// toolbar's height) for a heading that does not stick.
function stuckTop(entry, bar) {
  const top = parseFloat(getComputedStyle(entry.heading).top);
  return Number.isNaN(top) ? bar : top;
}
// The entry of the design system tab on screen: the last one whose heading
// has reached its place, and the last entry at the bottom of the page, where
// the last heading cannot reach it.
function systemCurrent(entries, bar) {
  const leaves = entries.flatMap(e => [e, ...e.subs]);
  if (!leaves.length) return null;
  if (window.innerHeight + window.scrollY >= document.documentElement.scrollHeight - 2) return leaves[leaves.length - 1];
  const reached = e => e.heading.getBoundingClientRect().top <= stuckTop(e, bar) + 2;
  const top = entries.filter(reached).pop() || entries[0];
  return top.subs.filter(reached).pop() || top;
}

// Draws <entries> in <nav>: each a link to <href> that calls <go>(entry) when
// clicked. Returns the function that marks an entry as the current one: its
// group opens to its sub-entries, and the other groups show one line each. The
// links stay in place when the mark moves, so a link that the keyboard is on
// keeps its focus.
function railView(nav, entries, href, go) {
  nav.replaceChildren();
  nav.hidden = !entries.length;
  const link = (entry, kind) => {
    const a = h("a", {class: kind, href}, entry.name);
    a.addEventListener("click", e => { e.preventDefault(); go(entry); });
    return a;
  };
  const groups = entries.map(entry => ({
    entry,
    a: link(entry, "grp"),
    subs: entry.subs.map(sub => ({entry: sub, a: link(sub, "sub")})),
  }));
  for (const g of groups) for (const s of g.subs) s.a.hidden = true;
  nav.append(h("div", {class: "k"}, "On this page"), ...groups.flatMap(g => [g.a, ...g.subs.map(s => s.a)]));
  const mark = (a, on) => {
    a.classList.toggle("cur", on);
    if (on) a.setAttribute("aria-current", "location"); else a.removeAttribute("aria-current");
  };
  let marked;
  return current => {
    if (current === marked) return;
    marked = current;
    for (const g of groups) {
      const open = g.entry === current || g.subs.some(s => s.entry === current);
      mark(g.a, open);
      for (const s of g.subs) {
        s.a.hidden = !open;
        mark(s.a, s.entry === current);
      }
    }
  };
}
