// Where the review page circles what the designer says changed on a board.
// Each circle is a CSS selector and an optional label. Each drawing of the
// board (now and proposed) answers, for each selector in order, the boxes (x,
// y, w, h in board pixels) of the elements it finds, or the error that stops
// the selector (board.html's `foundBy`). `circleMarks` takes the circles and
// each side's answers, null for a side the page has no drawing of, and returns
// the marks to draw on each side, each a box and the title the circle shows,
// and the problems to show above the board: a selector that is no selector,
// or one that finds nothing on any side.
function circleMarks(circles, now, proposed) {
  const out = {now: [], proposed: [], problems: []};
  circles.forEach((circle, i) => {
    const title = circle.label || circle.selector;
    const sides = [["now", now], ["proposed", proposed]].filter(([, found]) => found);
    const error = sides.map(([, found]) => found[i].error).find(e => e);
    if (error) {
      out.problems.push("The circle " + circle.selector + " is no CSS selector: " + error);
      return;
    }
    let boxes = 0;
    for (const [side, found] of sides) {
      for (const box of found[i].boxes) out[side].push({box, title});
      boxes += found[i].boxes.length;
    }
    if (!boxes) out.problems.push("The circle " + circle.selector + " finds no element on the board.");
  });
  return out;
}
