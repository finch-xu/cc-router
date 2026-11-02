#!/usr/bin/env node
// Desktop appearance preference checks; no browser or native WebView needed.
import assert from "node:assert/strict";
import { readUiZoom, applyUiZoom, UI_ZOOM_KEY } from "../src/lib/uiZoom.ts";

function memoryStorage(value = null) {
  const data = new Map(value === null ? [] : [[UI_ZOOM_KEY, value]]);
  return {
    getItem: (key) => data.get(key) ?? null,
    setItem: (key, next) => data.set(key, next),
  };
}

for (const raw of [null, "", "NaN", "0", "-90", "9000", "91", "{bad", "0.9"]) {
  assert.equal(readUiZoom(memoryStorage(raw)), 90, `invalid persisted zoom: ${raw}`);
}
const unavailable = {
  getItem() { throw new Error("storage denied"); },
  setItem() { throw new Error("storage denied"); },
};
assert.equal(readUiZoom(unavailable), 90);

for (const percent of [80, 90, 100, 110, 125]) {
  const storage = memoryStorage("90");
  let applied = null;
  const result = await applyUiZoom(percent, async (factor) => { applied = factor; }, storage);
  assert.equal(applied, percent / 100, "use absolute factor, with no Windows compensation");
  assert.equal(result.persisted, true);
  assert.equal(readUiZoom(storage), percent, "relaunch restores the applied value");
}

const storage = memoryStorage("100");
await assert.rejects(applyUiZoom(80, async () => { throw new Error("native failure"); }, storage), /native failure/);
assert.equal(readUiZoom(storage), 100, "failed application must not replace saved preference");

let appliedWithoutStorage = null;
assert.deepEqual(await applyUiZoom(110, async (factor) => { appliedWithoutStorage = factor; }, unavailable), { persisted: false });
assert.equal(appliedWithoutStorage, 1.1, "storage failure must not prevent current-session zoom");

for (const invalid of [0, 91, NaN, Infinity, "90"]) {
  let called = false;
  await assert.rejects(applyUiZoom(invalid, async () => { called = true; }, storage));
  assert.equal(called, false, "invalid zoom must not reach the native API");
}

console.log("check-ui-zoom: 全部通过");
