// Runs in a component's preview frame on a review page. The review page posts
// the CSS selectors of the elements it circles ({designCircles: [...]}, a
// null for a circle that is a box), and this script answers, once the preview
// has drawn, with the boxes in preview pixels of the elements each selector
// finds, or the error that stops it, as board.html does for a board
// ({designBoxes: {src, found}}). `src` names this frame to the page.
(function () {
  const drawn = new Promise(function (resolve) {
    const done = function () {
      (document.fonts ? document.fonts.ready : Promise.resolve()).then(function () {
        requestAnimationFrame(function () { requestAnimationFrame(resolve); });
      });
    };
    if (document.readyState === "complete") done(); else addEventListener("load", done);
  });
  function foundBy(selector) {
    let all;
    try {
      all = document.body.querySelectorAll(selector);
    } catch (error) {
      return {error: String((error && error.message) || error)};
    }
    const boxes = [];
    for (const el of all) {
      const r = el.getBoundingClientRect();
      if (r.width < 1 || r.height < 1) continue;
      boxes.push({x: Math.round(r.left + scrollX), y: Math.round(r.top + scrollY), w: Math.round(r.width), h: Math.round(r.height)});
    }
    return {boxes: boxes};
  }
  addEventListener("message", function (e) {
    const asked = e.data && e.data.designCircles;
    if (e.source !== parent || !Array.isArray(asked)) return;
    drawn.then(function () {
      parent.postMessage({designBoxes: {src: location.pathname, found: asked.map(function (s) { return s === null ? {boxes: []} : foundBy(s); })}}, "*");
    });
  });
})();
