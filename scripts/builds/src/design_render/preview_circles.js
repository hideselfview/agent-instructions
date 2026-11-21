// Runs in a component's preview frame on a review page, after preview_fit.js
// and circle_answers.js. The preview is drawn once the fit script has laid
// out a body that holds something (`previewLaidOut`) and the fonts are in.
answerCircles(function () { return document.body; },
  window.previewLaidOut.then(function () { return document.fonts ? document.fonts.ready : null; }));
