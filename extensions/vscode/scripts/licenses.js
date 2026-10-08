"use strict";
const fs = require("node:fs");
const path = require("node:path");
const { createRequire } = require("node:module");

// Follow the installed production graph rather than npm's view of unrelated
// packages in a reused cache. Package export maps may hide package.json.
function resolveManifest(name, parent) {
  const requireFrom = createRequire(parent);
  try { return requireFrom.resolve(`${name}/package.json`); }
  catch {
    let directory = path.dirname(requireFrom.resolve(name));
    for (let depth = 0; depth < 16; depth++) {
      const filename = path.join(directory, "package.json");
      if (fs.existsSync(filename) && JSON.parse(fs.readFileSync(filename, "utf8")).name === name) return filename;
      const ancestor = path.dirname(directory);
      if (ancestor === directory) break;
      directory = ancestor;
    }
    throw new Error(`Cannot resolve production license for ${name}`);
  }
}
function productionNotices(rootManifest) {
  const visited = new Map();
  function visit(filename, include) {
    filename = fs.realpathSync(filename);
    if (visited.has(filename)) return;
    if (visited.size >= 512) throw new Error("Production dependency graph exceeds bound");
    const manifest = JSON.parse(fs.readFileSync(filename, "utf8"));
    visited.set(filename, include ? manifest : undefined);
    for (const name of Object.keys(manifest.dependencies ?? {}).sort()) visit(resolveManifest(name, filename), true);
  }
  visit(path.resolve(rootManifest), false);
  return [...visited.entries()].filter(([, manifest]) => manifest).sort(([a], [b]) => a.localeCompare(b)).map(([filename, manifest]) => {
    const directory = path.dirname(filename);
    const licenseFile = fs.readdirSync(directory).find((name) => /^licen[cs]e(?:[.-]|$)/iu.test(name));
    return `${manifest.name}@${manifest.version}\nLicense: ${manifest.license || "see upstream license"}\n${licenseFile ? fs.readFileSync(path.join(directory, licenseFile), "utf8") : "No separate license file included by upstream package."}`;
  }).join("\n\n--------------------\n\n");
}
module.exports = { productionNotices };
