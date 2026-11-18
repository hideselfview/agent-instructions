// The controls both pages draw the same way; controls.css styles them.

// The button that dismisses a result: a cross, labelled for screen readers.
function dismissButton(onclick) {
  const b = document.createElement("button");
  b.type = "button";
  b.className = "bdismiss";
  b.title = "Dismiss this result";
  b.setAttribute("aria-label", "Dismiss");
  b.innerHTML = '<svg viewBox="0 0 16 16" width="12" height="12" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" aria-hidden="true"><path d="M4 4l8 8M12 4l-8 8"/></svg>';
  b.addEventListener("click", onclick);
  return b;
}
