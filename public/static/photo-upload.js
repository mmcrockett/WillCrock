(() => {
  const form = document.querySelector("#photo-upload");
  if (!form) return;
  const input = document.querySelector("#photo-files");
  const results = document.querySelector("#photo-upload-results");
  const button = form.querySelector("button");

  const request = async (url, options) => {
    const response = await fetch(url, {
      ...options,
      headers: { ...options.headers, "X-Willcrock-Request": "1" },
    });
    if (response.redirected) throw new Error("Please sign in again and retry.");
    const data = await response.json().catch(() => null);
    if (!response.ok || !data) throw new Error(data?.error || "Request failed. Please try again.");
    return data;
  };

  const showUpload = (file, uploaded, row) => {
    row.textContent = "";
    const message = document.createElement("p");
    message.textContent = uploaded.duplicate
      ? `${file.name}: Already in the library.`
      : `${file.name}: Uploaded. Date: ${uploaded.date} (${uploaded.date_source}).`;
    row.append(message);
    const gallery = document.createElement("a");
    const updateGallery = (date) => {
      if (!date) return;
      const month = date.slice(0, 7);
      gallery.href = `/photos/t/${month.slice(0, 4)}/${month}`;
      gallery.textContent = `View ${month} photos`;
    };
    updateGallery(uploaded.date);
    row.append(gallery);
    if (uploaded.duplicate) return;

    row.append(document.createElement("br"));
    const label = document.createElement("label");
    label.textContent = "Change year and month: ";
    const month = document.createElement("input");
    month.type = "month";
    month.value = uploaded.date.slice(0, 7);
    month.required = true;
    month.className = "form-control d-inline-block w-auto";
    label.append(month);
    const save = document.createElement("button");
    save.type = "button";
    save.className = "btn btn-sm btn-outline-secondary ms-2";
    save.textContent = "Save date";
    const status = document.createElement("span");
    status.className = "ms-2";
    save.addEventListener("click", async () => {
      if (!month.reportValidity()) return;
      save.disabled = true;
      try {
        await request(`/photos/i/${uploaded.id}/date`, {
          method: "POST",
          headers: { "Content-Type": "application/x-www-form-urlencoded" },
          body: new URLSearchParams({ month: month.value }),
        });
        status.textContent = "Date saved.";
        updateGallery(month.value);
      } catch (error) {
        status.textContent = error.message;
      } finally {
        save.disabled = false;
      }
    });
    row.append(label, save, status);
  };

  form.addEventListener("submit", async (event) => {
    event.preventDefault();
    const files = Array.from(input.files);
    if (!files.length) return;
    button.disabled = true;
    input.disabled = true;
    for (const file of files) {
      const row = document.createElement("div");
      row.className = "border-top pt-2 mt-2";
      row.textContent = `${file.name}: Uploading...`;
      results.append(row);
      try {
        if (file.size > 25 * 1024 * 1024) throw new Error("Photo exceeds 25 MB.");
        const uploaded = await request(`/photos/upload?filename=${encodeURIComponent(file.name)}`, {
          method: "POST",
          headers: { "Content-Type": "application/octet-stream" },
          body: file,
        });
        showUpload(file, uploaded, row);
      } catch (error) {
        row.textContent = `${file.name}: ${error.message}`;
      }
    }
    input.value = "";
    input.disabled = false;
    button.disabled = false;
  });
})();
