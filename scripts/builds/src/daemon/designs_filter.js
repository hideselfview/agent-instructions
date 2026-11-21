// The Designs page's Filter field: an entry stays when its title holds the
// text, ignoring case, and the field's text is already trimmed and lower case.
const matchesText = (title, words) => title.toLowerCase().includes(words);
// Hides the entries under <main> (the elements that carry data-name) that
// <words> does not match, and each part of the design system tab (a row of
// components, a subgroup, a group, the colors, the type) left with none. With
// no words everything shows. When no entry matches, <noMatch>() gives the
// element that says so. The tab keeps its frames, so typing loads none again.
function filterParts(main, words, noMatch) {
  for (const none of main.querySelectorAll("[data-none]")) none.remove();
  const items = main.querySelectorAll("[data-name]");
  let shown = 0;
  for (const item of items) {
    item.hidden = !matchesText(item.dataset.name, words);
    if (!item.hidden) shown++;
  }
  for (const part of main.querySelectorAll(".ds-cards, .ds-sub, .ds-group, .ds-swatches, .ds-type, .ds-sec"))
    part.hidden = !!words && ![...part.querySelectorAll("[data-name]")].some(item => !item.hidden);
  if (words && items.length && !shown) main.append(noMatch());
}
