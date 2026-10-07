"use strict";
const test = require("node:test");
const assert = require("node:assert/strict");
const { renderPreview } = require("../src/preview");
const doc = {
  source_id: "selected",
  path: "plan.md",
  revision: "a".repeat(64),
  line: 3,
};
test("preview escapes raw HTML and metadata and blocks scripts, images, frames, styles and arbitrary command URLs", () => {
  const html = renderPreview({
    ...doc,
    markdown:
      '# Title\n<script>PRIVATE_SCRIPT()</script>\n<img src="file:///private" onerror="alert(1)">\n\n![x](https://tracker/pixel)\n[bad](command:evil)\n[script](javascript:alert(1))\n[css](data:text/html,bad)\n[remote](//tracking/path)\n[good](https://example.org)\n',
  });
  assert.ok(html.includes("<h1>Title</h1>"));
  assert.ok(!html.includes("<script>"));
  assert.ok(!html.includes("<img"));
  assert.ok(!html.includes('href="command:evil'));
  assert.ok(!html.includes('href="javascript:'));
  assert.ok(!html.includes('href="data:'));
  assert.ok(!html.includes('href="//'));
  assert.ok(html.includes("default-src 'none'"));
  assert.ok(html.includes('href="https://example.org"'));
});
test("internal links carry only source-relative reference and revision arguments", () => {
  const html = renderPreview({ ...doc, markdown: "[Next](next.md#heading)\n" });
  const match = /href="command:mdreader.openReference\?([^"]+)"/u.exec(html);
  assert.ok(match);
  assert.deepEqual(JSON.parse(decodeURIComponent(match[1])), [
    {
      source_id: "selected",
      path: "plan.md",
      revision: doc.revision,
      target: "next.md#heading",
    },
  ]);
  assert.ok(html.includes("line 3"));
});
