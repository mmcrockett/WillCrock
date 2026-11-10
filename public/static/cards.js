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
