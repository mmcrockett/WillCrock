const assert = require("node:assert/strict");
const { readFileSync } = require("node:fs");
const { test } = require("node:test");
const vm = require("node:vm");

function setup(writeText, original) {
  let handler;
  let timer;
  const label = { textContent: original };
  const button = {
    dataset: { copy: "15551234567" },
    querySelector: () => label,
  };
  const context = {
    document: { addEventListener: (_, fn) => { handler = fn; } },
    navigator: { clipboard: { writeText } },
    setTimeout: (fn) => { timer = fn; return fn; },
    clearTimeout: (fn) => { if (fn === timer) timer = null; },
  };
  vm.runInNewContext(readFileSync("public/static/copy.js", "utf8"), context);
  return {
    label,
    click: () => handler({ target: { closest: () => button } }),
    expire: () => { const fn = timer; timer = null; fn(); },
  };
}

test("copy reports success only after writing and restores the original label", async () => {
  let finish;
  let copied;
  const ui = setup((value) => {
    copied = value;
    return new Promise((resolve) => { finish = resolve; });
  }, "");
  const pending = ui.click();
  assert.equal(copied, "15551234567");
  assert.equal(ui.label.textContent, "");
  finish();
  await pending;
  assert.equal(ui.label.textContent, "Copied");
  ui.expire();
  assert.equal(ui.label.textContent, "");
});

test("copy failures are visible and code labels are restored", async () => {
  const ui = setup(() => Promise.reject(new Error("denied")), "123-456");
  await ui.click();
  assert.equal(ui.label.textContent, "Copy failed");
  ui.expire();
  assert.equal(ui.label.textContent, "123-456");
});

test("an unavailable clipboard is reported as a failure", async () => {
  const ui = setup(undefined, "Copy");
  await ui.click();
  assert.equal(ui.label.textContent, "Copy failed");
  ui.expire();
  assert.equal(ui.label.textContent, "Copy");
});

test("repeated clicks ignore stale results and restore the original label", async () => {
  const pending = [];
  const ui = setup(() => new Promise((resolve, reject) => {
    pending.push({ resolve, reject });
  }), "Copy");
  const first = ui.click();
  const second = ui.click();
  pending[1].resolve();
  await second;
  pending[0].reject(new Error("denied"));
  await first;
  assert.equal(ui.label.textContent, "Copied");
  ui.expire();
  assert.equal(ui.label.textContent, "Copy");
});
