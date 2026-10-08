"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const os = require("node:os");
const { productionNotices } = require("../scripts/licenses");

test("license notices follow actual symlinked production dependencies and exclude cached development packages", (t) => {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), "mdreader-license-fixture-"));
  t.after(() => fs.rmSync(root, { recursive: true, force: true }));
  const project = path.join(root, "project");
  const cache = path.join(root, "cache");
  fs.mkdirSync(project);
  fs.mkdirSync(path.join(cache, "selected/node_modules/transitive"), { recursive: true });
  fs.mkdirSync(path.join(cache, "development"));
  fs.writeFileSync(path.join(project, "package.json"), JSON.stringify({ name: "fixture", dependencies: { selected: "1" }, devDependencies: { development: "1" } }));
  fs.writeFileSync(path.join(cache, "selected/package.json"), JSON.stringify({ name: "selected", version: "1.0.0", license: "MIT", dependencies: { transitive: "1" } }));
  fs.writeFileSync(path.join(cache, "selected/LICENSE"), "SYNTHETIC_SELECTED_LICENSE");
  fs.writeFileSync(path.join(cache, "selected/node_modules/transitive/package.json"), JSON.stringify({ name: "transitive", version: "1.0.0", license: "MIT" }));
  fs.writeFileSync(path.join(cache, "development/package.json"), JSON.stringify({ name: "development", version: "1.0.0", license: "PRIVATE_DEV_SENTINEL" }));
  fs.symlinkSync(cache, path.join(project, "node_modules"), "dir");
  const notices = productionNotices(path.join(project, "package.json"));
  assert.ok(notices.includes("selected@1.0.0"));
  assert.ok(notices.includes("SYNTHETIC_SELECTED_LICENSE"));
  assert.ok(notices.includes("transitive@1.0.0"));
  assert.ok(!notices.includes("development"));
  assert.ok(!notices.includes("PRIVATE_DEV_SENTINEL"));
});
