#!/usr/bin/env node
//
// gen-corpus.mjs — deterministic synthetic markdown corpus generator.
//
//   node scripts/gen-corpus.mjs [outdir] [count]
//
//   outdir  where to write files      (default: fixtures/generated)
//   count   number of regular docs    (default: 40)
//
// Output: `count` regular docs (doc-0001.md ... doc-NNNN.md) with varied
// headings, links and sizes, PLUS one ~5 MB document (huge-doc.md).
//
// Determinism: every byte is a pure function of a fixed base seed and the
// file's index — there is NO Date.now(), Math.random(), or other ambient
// input. Running twice (into two directories, or the same one) yields
// byte-identical output, so the two trees `diff` clean. Zero dependencies:
// only Node's built-in `fs`/`path`.
//
// The output is disposable and git-ignored (fixtures/generated/); do not
// commit it. Hand-authored fixtures live in fixtures/corpus/ instead.

import fs from "node:fs";
import path from "node:path";

const BASE_SEED = 0x9e3779b9; // fixed — golden-ratio constant, not the clock
const HUGE_TARGET_BYTES = 5 * 1024 * 1024; // ~5 MB

// --- deterministic PRNG (mulberry32) -------------------------------------
function mulberry32(seed) {
  let a = seed >>> 0;
  return function () {
    a |= 0;
    a = (a + 0x6d2b79f5) | 0;
    let t = Math.imul(a ^ (a >>> 15), 1 | a);
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t;
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}

// --- word banks (fixed, so content is reproducible) ----------------------
const NOUNS = [
  "ingest", "storage", "embedding", "vector", "index", "provenance", "query",
  "planner", "graph", "search", "corpus", "chunk", "token", "migration",
  "backend", "cache", "schema", "trigram", "cluster", "pipeline", "document",
  "heading", "anchor", "frontmatter", "checkpoint", "shard", "manifest",
];
const VERBS = [
  "ingests", "indexes", "embeds", "ranks", "traverses", "parses", "resolves",
  "caches", "streams", "compacts", "validates", "rewrites", "merges", "scans",
];
const ADJS = [
  "content-addressed", "hybrid", "incremental", "deterministic", "lexical",
  "semantic", "nested", "sparse", "dense", "immutable", "append-only",
];
const LANGS = ["rust", "python", "typescript", "sql", "bash", "json", "toml"];

function pick(rng, arr) {
  return arr[Math.floor(rng() * arr.length) % arr.length];
}
function randInt(rng, lo, hi) {
  return lo + Math.floor(rng() * (hi - lo + 1));
}

function sentence(rng) {
  const n = randInt(rng, 6, 16);
  const words = [];
  for (let i = 0; i < n; i++) {
    const r = rng();
    if (r < 0.3) words.push(pick(rng, ADJS));
    else if (r < 0.65) words.push(pick(rng, NOUNS));
    else words.push(pick(rng, VERBS));
  }
  const s = words.join(" ");
  return s.charAt(0).toUpperCase() + s.slice(1) + ".";
}
function paragraph(rng) {
  const n = randInt(rng, 2, 6);
  const out = [];
  for (let i = 0; i < n; i++) out.push(sentence(rng));
  return out.join(" ");
}

function codeFence(rng) {
  const lang = pick(rng, LANGS);
  const body = {
    rust: "fn main() {\n    let n = 42;\n    println!(\"{n}\");\n}",
    python: "def f(x):\n    return x * 2",
    typescript: "export const id = (x: number): number => x;",
    sql: "select id from notes order by embedding <-> $1 limit 10;",
    bash: "set -euo pipefail\ncargo test --workspace",
    json: '{ "ok": true, "count": 3 }',
    toml: "[db]\nport = 54322",
  }[lang];
  return "```" + lang + "\n" + body + "\n```";
}

// Build one regular document. `idx` is 1-based; `count` is the total so links
// stay in range.
function buildDoc(idx, count) {
  const rng = mulberry32((BASE_SEED + idx) >>> 0);
  const id = String(idx).padStart(4, "0");
  const tagA = pick(rng, NOUNS);
  const tagB = pick(rng, ADJS);

  let out = "";
  out += "---\n";
  out += `title: "Doc ${id}: ${pick(rng, ADJS)} ${pick(rng, NOUNS)}"\n`;
  out += `id: doc-${id}\n`;
  out += `tags: [generated, ${tagA}, ${tagB}]\n`;
  out += `weight: ${randInt(rng, 1, 100)}\n`;
  out += "---\n\n";

  out += `# Doc ${id}\n\n`;
  out += paragraph(rng) + "\n\n";

  const sections = randInt(rng, 2, 8);
  for (let s = 0; s < sections; s++) {
    const level = randInt(rng, 2, 6); // h2..h6, varied nesting
    out += "#".repeat(level) + " " + sentence(rng).replace(/\.$/, "") + "\n\n";

    const paras = randInt(rng, 1, 4);
    for (let p = 0; p < paras; p++) out += paragraph(rng) + "\n\n";

    const r = rng();
    if (r < 0.25) {
      // list
      const items = randInt(rng, 2, 5);
      for (let i = 0; i < items; i++) out += `- ${sentence(rng)}\n`;
      out += "\n";
    } else if (r < 0.45) {
      out += codeFence(rng) + "\n\n";
    }

    // Cross links: a relative link to another doc + a wiki-link.
    if (count > 1 && rng() < 0.7) {
      let other = randInt(rng, 1, count);
      if (other === idx) other = (other % count) + 1;
      const oid = String(other).padStart(4, "0");
      out += `See [doc ${oid}](doc-${oid}.md) and [[doc-${oid}]].\n\n`;
    }
  }
  return { name: `doc-${id}.md`, content: out };
}

// Build the ~5 MB document deterministically.
function buildHuge() {
  const rng = mulberry32((BASE_SEED ^ 0x5bd1e995) >>> 0);
  let out = "";
  out += "---\n";
  out += 'title: "Huge generated document"\n';
  out += "id: huge-doc\n";
  out += "tags: [generated, large]\n";
  out += "---\n\n";
  out += "# Huge Document\n\n";
  out += "Synthetic ~5 MB document for large-file handling tests.\n\n";

  let section = 0;
  while (Buffer.byteLength(out, "utf8") < HUGE_TARGET_BYTES) {
    section++;
    out += `## Section ${section}: ${sentence(rng).replace(/\.$/, "")}\n\n`;
    const paras = randInt(rng, 3, 6);
    for (let p = 0; p < paras; p++) out += paragraph(rng) + "\n\n";
    if (rng() < 0.2) out += codeFence(rng) + "\n\n";
    if (rng() < 0.3) {
      out += `- ${sentence(rng)}\n- ${sentence(rng)}\n- ${sentence(rng)}\n\n`;
    }
  }
  out += "\n## End\n\nEnd of the huge document.\n";
  return { name: "huge-doc.md", content: out };
}

// --- main ----------------------------------------------------------------
function main() {
  const outdir = process.argv[2] || "fixtures/generated";
  const count = Number.parseInt(process.argv[3] ?? "40", 10);
  if (!Number.isInteger(count) || count < 1) {
    console.error(`invalid count: ${process.argv[3]}`);
    process.exit(1);
  }

  fs.mkdirSync(outdir, { recursive: true });

  // Clean only files this generator owns, so re-running into the same dir is
  // idempotent without disturbing anything else that happens to live there.
  for (const f of fs.readdirSync(outdir)) {
    if (/^doc-\d+\.md$/.test(f) || f === "huge-doc.md") {
      fs.rmSync(path.join(outdir, f));
    }
  }

  const docs = [];
  for (let i = 1; i <= count; i++) docs.push(buildDoc(i, count));
  docs.push(buildHuge());

  for (const d of docs) {
    fs.writeFileSync(path.join(outdir, d.name), d.content);
  }

  const huge = docs[docs.length - 1];
  const hugeBytes = Buffer.byteLength(huge.content, "utf8");
  console.log(
    `Wrote ${count} docs + huge-doc.md (${(hugeBytes / 1048576).toFixed(2)} MiB) to ${outdir}`,
  );
}

main();
