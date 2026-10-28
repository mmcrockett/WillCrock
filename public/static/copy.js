document.addEventListener("click", function (e) {
  var b = e.target.closest("[data-copy]");
  if (!b) return;
  navigator.clipboard.writeText(b.dataset.copy);
  var t = b.textContent;
  b.textContent = "Copied";
  setTimeout(function () {
    b.textContent = t;
  }, 900);
});
