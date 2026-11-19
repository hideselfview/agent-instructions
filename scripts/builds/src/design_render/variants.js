// Where the review page crops each drawing of a design change's variants:
// the box, in board pixels, of the first element the variants' selector finds
// on the board, with VARIANT_MARGIN pixels around it, kept within the board (w
// by h). `found` is the board page's answer for the selector (board.html's
// `foundBy`): the boxes of the elements a person sees, or the error that stops
// the selector. `variantCrop` returns the crop as {crop: {x, y, w, h}}, or
// {problem} to show above the whole board when there is nothing to crop to.
const VARIANT_MARGIN = 16;
function variantCrop(selector, found, w, h) {
  if (found.error) return {problem: "The selector " + selector + " is no CSS selector: " + found.error};
  const first = found.boxes[0];
  if (!first) return {problem: "The selector " + selector + " finds no element on this variant."};
  const x = Math.max(0, first.x - VARIANT_MARGIN);
  const y = Math.max(0, first.y - VARIANT_MARGIN);
  const right = Math.min(w, first.x + first.w + VARIANT_MARGIN);
  const bottom = Math.min(h, first.y + first.h + VARIANT_MARGIN);
  if (right <= x || bottom <= y) return {problem: "The selector " + selector + " finds an element outside the board."};
  return {crop: {x, y, w: right - x, h: bottom - y}};
}
