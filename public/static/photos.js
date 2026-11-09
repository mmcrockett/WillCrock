(() => {
  const tagPicker = document.querySelector(".photo-tag-picker");
  if (tagPicker) {
    const form = tagPicker.querySelector("[data-tag-form]");
    const tagsInput = form.querySelector("[name=tags]");
    const returnToInput = form.querySelector("[name=return_to]");
    const newTagInput = form.querySelector("#new-photo-tag");
    const options = () => Array.from(form.querySelectorAll("[name=known_tag]"));
    const buttons = Array.from(document.querySelectorAll("[data-tag-picker]"));
    const tagOptions = form.querySelector(".photo-tag-options");

    buttons.forEach((button) => {
      button.addEventListener("click", () => {
        form.action = button.dataset.action;
        returnToInput.value = button.dataset.returnTo;
        const selected = new Set(button.dataset.tags.split(", ").filter(Boolean));
        tagOptions.querySelectorAll("[data-new-tag-option]").forEach((option) => option.remove());
        options().forEach((option) => { option.checked = selected.has(option.value); });
        newTagInput.value = "";
        tagPicker.showModal();
      });
    });

    const addTags = (focusInput = true) => {
      const tags = newTagInput.value.split(",").map((tag) => tag.trim().toLowerCase()).filter(Boolean);
      tags.forEach((tag) => {
        const existing = options().find((option) => option.value.toLowerCase() === tag);
        if (existing) {
          existing.checked = true;
          return;
        }
        const label = document.createElement("label");
        label.className = "photo-tag-option";
        label.dataset.newTagOption = "";
        const checkbox = document.createElement("input");
        checkbox.className = "visually-hidden";
        checkbox.type = "checkbox";
        checkbox.name = "known_tag";
        checkbox.value = tag;
        checkbox.checked = true;
        const text = document.createElement("span");
        text.textContent = tag;
        label.append(checkbox, text);
        tagOptions.append(label);
      });
      newTagInput.value = "";
      if (focusInput) newTagInput.focus();
    };

    tagPicker.querySelector("[data-tag-add]").addEventListener("click", addTags);
    newTagInput.addEventListener("keydown", (event) => {
      if (event.isComposing || event.key !== "Enter") return;
      event.preventDefault();
      addTags();
    });
    form.addEventListener("submit", () => {
      if (newTagInput.value.trim()) addTags(false);
      const selected = options().filter((option) => option.checked).map((option) => option.value);
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

  document.querySelectorAll("[data-share-link]").forEach((input) => {
    input.value = new URL(input.value, window.location.origin).href;
    input.addEventListener("click", () => input.select());
  });
  const shareForm = document.querySelector("#share-album");
  if (shareForm) {
    const key = "willcrock-share-selection";
    let selected = new Set();
    try {
      if (new URLSearchParams(window.location.search).has("created")) sessionStorage.removeItem(key);
      selected = new Set(JSON.parse(sessionStorage.getItem(key) || "[]"));
    } catch (error) {
      console.error("Could not restore album selection", error);
    }
    const ids = document.createElement("input");
    ids.type = "hidden";
    ids.name = "photo_ids";
    shareForm.append(ids);
    const checkboxes = document.querySelectorAll("[data-share-photo]");
    const update = () => {
      ids.value = Array.from(selected).join(",");
      shareForm.querySelector("[data-share-selection]").hidden = selected.size === 0;
      shareForm.querySelector("[data-share-count]").textContent = selected.size;
      try {
        sessionStorage.setItem(key, JSON.stringify(Array.from(selected)));
      } catch (error) {
        console.error("Could not save album selection", error);
      }
    };
    checkboxes.forEach((checkbox) => {
      checkbox.checked = selected.has(checkbox.dataset.sharePhoto);
      checkbox.addEventListener("change", () => {
        if (checkbox.checked) selected.add(checkbox.dataset.sharePhoto);
        else selected.delete(checkbox.dataset.sharePhoto);
        update();
      });
    });
    shareForm.querySelector("[data-share-clear]").addEventListener("click", () => {
      selected.clear();
      checkboxes.forEach((checkbox) => { checkbox.checked = false; });
      update();
    });
    update();
  } else if (document.querySelector("[data-share-link]") &&
             new URLSearchParams(window.location.search).has("created")) {
    try {
      sessionStorage.removeItem("willcrock-share-selection");
    } catch (error) {
      console.error("Could not clear album selection", error);
    }
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
