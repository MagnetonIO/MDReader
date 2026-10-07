"use strict";
const MarkdownIt = require("markdown-it");
const sanitize = require("sanitize-html");
const { validateSelection } = require("./reader-client");
const escape = (value) =>
  String(value).replace(
    /[&<>"']/gu,
    (c) =>
      ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[
        c
      ],
  );
function renderPreview(document) {
  const selected = validateSelection(document, true);
  const markdown = new MarkdownIt({
    html: false,
    linkify: false,
    typographer: false,
  });
  const link =
    markdown.renderer.rules.link_open ||
    ((tokens, index, options, env, self) =>
      self.renderToken(tokens, index, options));
  markdown.renderer.rules.link_open = (tokens, index, options, env, self) => {
    const token = tokens[index];
    const target = token.attrGet("href") || "";
    if (
      !/^[a-z][a-z0-9+.-]*:/iu.test(target) &&
      !target.startsWith("/") &&
      !target.includes("\\")
    ) {
      const args = [
        {
          source_id: selected.source_id,
          path: selected.path,
          revision: selected.revision,
          target,
        },
      ];
      token.attrSet(
        "href",
        `command:mdreader.openReference?${encodeURIComponent(JSON.stringify(args))}`,
      );
    } else if (!/^https?:\/\//iu.test(target)) token.attrSet("href", "#");
    token.attrSet("rel", "noreferrer noopener");
    return link(tokens, index, options, env, self);
  };
  // Images and all resource-loading markup are excluded until the provider has
  // an explicit, authenticated asset contract. Raw HTML never enters the DOM.
  markdown.renderer.rules.image = () =>
    '<span class="asset">[Image not loaded]</span>';
  const body = sanitize(markdown.render(document.markdown), {
    allowedTags: [
      "p",
      "h1",
      "h2",
      "h3",
      "h4",
      "h5",
      "h6",
      "blockquote",
      "pre",
      "code",
      "em",
      "strong",
      "s",
      "ul",
      "ol",
      "li",
      "hr",
      "br",
      "a",
      "table",
      "thead",
      "tbody",
      "tr",
      "th",
      "td",
      "span",
    ],
    allowedAttributes: {
      a: ["href", "title", "rel"],
      code: ["class"],
      span: ["class"],
      ol: ["start"],
    },
    allowedSchemes: ["http", "https", "command"],
    allowedSchemesByTag: { a: ["http", "https", "command"] },
    allowProtocolRelative: false,
    transformTags: {
      a: (tagName, attributes) => {
        // The only command URL accepted is the one constructed by our renderer.
        if (
          attributes.href?.startsWith("command:") &&
          !attributes.href.startsWith("command:mdreader.openReference?")
        )
          delete attributes.href;
        return { tagName, attribs: attributes };
      },
    },
  });
  return `<!DOCTYPE html><html><head><meta charset="utf-8"><meta http-equiv="Content-Security-Policy" content="default-src 'none'; style-src 'unsafe-inline'; img-src 'none'; frame-src 'none'; base-uri 'none'; form-action 'none'"><meta name="viewport" content="width=device-width,initial-scale=1"><style>body{font-family:var(--vscode-font-family);color:var(--vscode-editor-foreground);background:var(--vscode-editor-background);padding:20px;max-width:860px;margin:auto;line-height:1.6}pre,code{font-family:var(--vscode-editor-font-family);background:var(--vscode-textCodeBlock-background)}pre{padding:12px;overflow:auto}a{color:var(--vscode-textLink-foreground)}table{border-collapse:collapse}td,th{border:1px solid var(--vscode-panel-border);padding:6px}.evidence,.asset{color:var(--vscode-descriptionForeground);font-size:.9em}.evidence{overflow-wrap:anywhere}</style></head><body><p class="evidence">${escape(selected.source_id)} / ${escape(selected.path)} · revision ${escape(selected.revision)} · line ${selected.line}</p><main>${body}</main></body></html>`;
}
module.exports = { renderPreview };
