var copyStates = new WeakMap();

document.addEventListener("click", async function (e) {
  var b = e.target.closest("[data-copy]");
  if (!b) return;
  var label = b.querySelector("[data-copy-label]");
  var state = copyStates.get(b);
  if (!state) {
    state = { original: label.textContent, request: 0, timer: null };
    copyStates.set(b, state);
  }
  var request = ++state.request;
  clearTimeout(state.timer);
  try {
    await navigator.clipboard.writeText(b.dataset.copy);
    if (request !== state.request) return;
    label.textContent = "Copied";
  } catch (error) {
    if (request !== state.request) return;
    label.textContent = "Copy failed";
  }
  state.timer = setTimeout(function () {
    label.textContent = state.original;
  }, 1500);
});
