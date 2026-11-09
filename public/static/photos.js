(() => {
  const tagPicker = document.querySelector(".photo-tag-picker");
  if (tagPicker) {
    const form = tagPicker.querySelector("[data-tag-form]");
    const tagsInput = form.querySelector("[name=tags]");
    const returnToInput = form.querySelector("[name=return_to]");
    const newTagInput = form.querySelector("[name=new_tag]");
    const options = Array.from(form.querySelectorAll("[name=known_tag]"));
    const buttons = Array.from(document.querySelectorAll("[data-tag-picker]"));

    buttons.forEach((button) => {
      button.addEventListener("click", () => {
        form.action = button.dataset.action;
        returnToInput.value = button.dataset.returnTo;
        const selected = new Set(button.dataset.tags.split(", ").filter(Boolean));
        options.forEach((option) => { option.checked = selected.has(option.value); });
        newTagInput.value = "";
        tagPicker.showModal();
      });
    });

    form.addEventListener("submit", () => {
      const selected = options.filter((option) => option.checked).map((option) => option.value);
      if (newTagInput.value.trim()) selected.push(newTagInput.value.trim());
      tagsInput.value = selected.join(", ");
    });
    tagPicker.querySelector("[data-tag-close]").addEventListener("click", () => tagPicker.close());
    tagPicker.addEventListener("click", (event) => {
      if (event.target !== tagPicker) return;
      const bounds = tagPicker.getBoundingClientRect();
      if (event.clientX < bounds.left || event.clientX > bounds.right ||
          event.clientY < bounds.top || event.clientY > bounds.bottom) tagPicker.close();
    });
  }

  const viewer = document.querySelector(".photo-viewer");
  if (!viewer) return;
  const tiles = Array.from(document.querySelectorAll("[data-photo]"));
  const image = viewer.querySelector(".photo-viewer-image");
  const caption = viewer.querySelector(".photo-viewer-caption");
  const error = viewer.querySelector(".photo-viewer-error");
  const download = viewer.querySelector("[data-photo-download]");
  const previous = viewer.querySelector("[data-photo-prev]");
  const next = viewer.querySelector("[data-photo-next]");
  let index = 0;

  const show = (position) => {
    index = position;
    const tile = tiles[index];
    image.hidden = true;
    error.hidden = true;
    image.alt = tile.dataset.caption;
    caption.textContent = `${tile.dataset.caption} (${index + 1} of ${tiles.length} on this page)`;
    download.href = tile.dataset.original;
    previous.disabled = index === 0;
    next.disabled = index === tiles.length - 1;
    image.src = tile.href;
  };

  image.addEventListener("load", () => { image.hidden = false; });
  image.addEventListener("error", () => {
    image.hidden = true;
    error.hidden = false;
  });
  tiles.forEach((tile, position) => {
    const thumb = tile.querySelector("img");
    const failed = () => {
      thumb.hidden = true;
      tile.querySelector(".photo-load-error").hidden = false;
    };
    thumb.addEventListener("error", failed);
    if (thumb.complete && thumb.naturalWidth === 0) failed();
    tile.addEventListener("click", (event) => {
      if (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey) return;
      if (typeof viewer.showModal !== "function") return;
      event.preventDefault();
      show(position);
      viewer.showModal();
    });
  });
  previous.addEventListener("click", () => { if (index > 0) show(index - 1); });
  next.addEventListener("click", () => { if (index < tiles.length - 1) show(index + 1); });
  viewer.querySelector("[data-photo-close]").addEventListener("click", () => viewer.close());
  viewer.addEventListener("click", (event) => {
    if (event.target !== viewer) return;
    const bounds = viewer.getBoundingClientRect();
    if (event.clientX < bounds.left || event.clientX > bounds.right ||
        event.clientY < bounds.top || event.clientY > bounds.bottom) viewer.close();
  });
  viewer.addEventListener("keydown", (event) => {
    if (event.key === "ArrowLeft" && index > 0) {
      event.preventDefault();
      show(index - 1);
    } else if (event.key === "ArrowRight" && index < tiles.length - 1) {
      event.preventDefault();
      show(index + 1);
    }
  });
  viewer.addEventListener("close", () => {
    image.removeAttribute("src");
    tiles[index].focus();
  });
})();
