"use strict";
const fs = require("node:fs");
const path = require("node:path");
const { execFileSync } = require("node:child_process");
require("esbuild").buildSync({
  entryPoints: ["src/extension.js"],
  bundle: true,
  platform: "node",
  target: "node20",
  external: ["vscode"],
  outfile: "dist/extension.js",
});
fs.mkdirSync("artifacts", { recursive: true });
const dependencies = execFileSync(
  process.platform === "win32" ? "npm.cmd" : "npm",
  ["ls", "--omit=dev", "--parseable", "--all"],
  { encoding: "utf8" },
)
  .trim()
  .split("\n")
  .slice(1);
const notices = [];
for (const directory of [...new Set(dependencies)].sort()) {
  const manifest = JSON.parse(
    fs.readFileSync(path.join(directory, "package.json"), "utf8"),
  );
  const licenseFile = fs
    .readdirSync(directory)
    .find((name) => /^licen[cs]e(?:[.-]|$)/iu.test(name));
  notices.push(
    `${manifest.name}@${manifest.version}\nLicense: ${manifest.license || "see upstream license"}\n${licenseFile ? fs.readFileSync(path.join(directory, licenseFile), "utf8") : "No separate license file included by upstream package."}`,
  );
}
fs.writeFileSync(
  "dist/THIRD_PARTY_NOTICES.txt",
  notices.join("\n\n--------------------\n\n"),
);
