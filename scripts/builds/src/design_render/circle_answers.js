// Runs in a page that a review page frames, a board's or a component's
// preview. The review page posts the CSS selectors of the elements it
// circles ({designCircles: [...]}, a null for a circle that is a box), and
// `answerCircles` answers, once <drawn> has resolved, with the boxes in page
// pixels of the elements each selector finds inside the element <root>
// returns, or the error that stops the selector ({designBoxes: {src,
// found}}). `src` names this frame to the page.
function answerCircles(root, drawn) {
  function foundBy(selector) {
    var all;
    try {
      all = root().querySelectorAll(selector);
    } catch (error) {
      return { error: String(error && error.message || error) };
    }
    var boxes = [];
    Array.prototype.forEach.call(all, function (el) {
      var r = el.getBoundingClientRect();
      if (r.width < 1 || r.height < 1) return;
      boxes.push({ x: Math.round(r.left + window.scrollX), y: Math.round(r.top + window.scrollY), w: Math.round(r.width), h: Math.round(r.height) });
    });
    return { boxes: boxes };
  }
  window.addEventListener('message', function (e) {
    var asked = e.data && e.data.designCircles;
    if (e.source !== parent || !Array.isArray(asked)) return;
    drawn.then(function () {
      parent.postMessage({ designBoxes: { src: location.pathname, found: asked.map(function (s) { return s === null ? { boxes: [] } : foundBy(s); }) } }, '*');
    });
  });
}
