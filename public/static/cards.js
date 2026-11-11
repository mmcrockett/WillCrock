document.querySelectorAll("[data-card-upload]").forEach((form) => {
  const name = form.querySelector('[name="name"]');
  const input = form.querySelector('input[type="file"]');
  const status = form.querySelector('[role="status"]');
  const button = form.querySelector("button");

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const file = input.files[0];
    if (!file) return;
    button.disabled = true;
    input.disabled = true;
    if (name) name.disabled = true;
    status.textContent = "Uploading...";
    try {
      if (name && !name.value.trim()) throw new Error("Card name is required.");
      if (file.size > 25 * 1024 * 1024) throw new Error("Card image exceeds 25 MB.");
      const body = new FormData();
      body.append("photo", file, file.name);
      const url = new URL(form.dataset.uploadUrl, window.location.origin);
      if (name) url.searchParams.set("name", name.value);
      const response = await fetch(url, {
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
      if (name) name.disabled = false;
    }
  });
});

document.querySelectorAll(".card-side[data-side]").forEach((panel) => {
  const display = panel.querySelector(".card-display");
  const image = display.querySelector("img");
  const controls = panel.querySelector('[id^="card-size-controls"]');
  const slider = panel.querySelector('input[type="range"]');
  const value = panel.querySelector("output");
  const status = panel.querySelector('[role="status"]');
  const key = `willcrock:card-size:${display.dataset.cardId}${panel.dataset.side === "Back" ? ":back" : ""}`;
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
});
