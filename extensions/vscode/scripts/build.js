"use strict";
const fs = require("node:fs");
const path = require("node:path");
const { productionNotices } = require("./licenses");
require("esbuild").buildSync({
  entryPoints: ["src/extension.js"],
  bundle: true,
  platform: "node",
  target: "node20",
  external: ["vscode"],
  outfile: "dist/extension.js",
});
fs.mkdirSync("artifacts", { recursive: true });
fs.writeFileSync(
  "dist/THIRD_PARTY_NOTICES.txt",
  productionNotices(path.resolve("package.json")),
);
