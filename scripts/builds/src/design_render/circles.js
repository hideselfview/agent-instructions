// Where the review page circles what the designer says changed on a board.
// Each circle is a CSS selector or a box (x, y, width, height in board
// pixels), an optional label and the name the core gives it. A box is marked on
// each drawing that has answered. Each drawing of the board (now and
// proposed) answers, for each selector in order, the boxes (x,
// y, w, h in board pixels) of the elements it finds, or the error that stops
// the selector (board.html's `foundBy`). `circleMarks` takes the circles and
// each side's answers, null for a side with no drawing or no answer yet, and
// returns the marks to draw on each side, each a box and the title the circle
// shows, and the problems to show above the board: a selector that is no
// selector, or one that finds nothing on any side. A side that has not
// answered is null and gets no marks, so each side draws its marks when its
// own drawing answers; the page shows the problems once every drawing has.
function circleMarks(circles, now, proposed) {
  const out = {now: [], proposed: [], problems: []};
  circles.forEach((circle, i) => {
    const sides = [["now", now], ["proposed", proposed]].filter(([, found]) => found);
    if (circle.box) {
      const b = circle.box;
      const title = circle.label || circle.name;
      for (const [side] of sides) out[side].push({box: {x: b.x, y: b.y, w: b.width, h: b.height}, title});
      return;
    }
    const title = circle.label || circle.name;
    const error = sides.map(([, found]) => found[i].error).find(e => e);
    if (error) {
      out.problems.push("The circle " + circle.name + " is no CSS selector: " + error);
      return;
    }
    let boxes = 0;
    for (const [side, found] of sides) {
      for (const box of found[i].boxes) out[side].push({box, title});
      boxes += found[i].boxes.length;
    }
    if (!boxes) out.problems.push("The circle " + circle.name + " finds no element on the board.");
  });
  return out;
}
