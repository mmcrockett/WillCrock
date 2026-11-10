(() => {
  const form = document.querySelector("#card-upload");
  if (!form) return;
  const name = document.querySelector("#card-name");
  const input = document.querySelector("#card-file");
  const status = document.querySelector("#card-upload-status");
  const button = form.querySelector("button");

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const file = input.files[0];
    if (!file) return;
    button.disabled = true;
    input.disabled = true;
    name.disabled = true;
    status.textContent = "Uploading...";
    try {
      if (!name.value.trim()) throw new Error("Card name is required.");
      if (file.size > 25 * 1024 * 1024) throw new Error("Card image exceeds 25 MB.");
      const body = new FormData();
      body.append("photo", file, file.name);
      const response = await fetch(`/cards/upload?${new URLSearchParams({ name: name.value })}`, {
        method: "POST",
        headers: { "X-Willcrock-Request": "1" },
        body,
      });
      if (response.redirected) throw new Error("Please sign in again and retry.");
      const data = await response.json().catch(() => null);
      if (!response.ok || !data) throw new Error(data?.error || "Upload failed. Please try again.");
      window.location.assign(`/cards/${data.id}`);
    } catch (error) {
      status.textContent = error.message;
    } finally {
      button.disabled = false;
      input.disabled = false;
      name.disabled = false;
    }
  });
})();

(() => {
  const display = document.querySelector(".card-display");
  if (!display) return;
  const image = display.querySelector("img");
  const controls = document.querySelector("#card-size-controls");
  const slider = document.querySelector("#card-size");
  const value = document.querySelector("#card-size-value");
  const status = document.querySelector("#card-size-status");
  const key = `willcrock:card-size:${display.dataset.cardId}`;
  const storageError = () => {
    status.textContent = "Size can't be remembered in this browser.";
  };
  const update = () => {
    display.style.setProperty("--card-size", `${slider.value}%`);
    value.value = `${slider.value}%`;
  };
  const initialize = () => {
    if (!image.naturalWidth) return;
    display.style.maxWidth = `${image.naturalWidth}px`;
    display.style.aspectRatio = `${image.naturalWidth} / ${image.naturalHeight}`;
    display.classList.add("card-sized");
    try {
      const saved = Number(localStorage.getItem(key));
      if (saved >= 25 && saved <= 100 && saved % 5 === 0) slider.value = saved;
    } catch {
      storageError();
    }
    update();
    controls.hidden = false;
  };
  slider.addEventListener("input", update);
  slider.addEventListener("change", () => {
    try {
      localStorage.setItem(key, slider.value);
    } catch {
      storageError();
    }
  });
  if (image.complete) initialize();
  else image.addEventListener("load", initialize, { once: true });
})();
