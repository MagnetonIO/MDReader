"use strict";
const fs = require("node:fs/promises");
const path = require("node:path");
const net = require("node:net");
const { TextDecoder } = require("node:util");
const MAX_RESPONSE = 16 * 1024 * 1024;
const MAX_REQUEST = 64 * 1024;
const REVISION = /^[a-fA-F0-9]{64}$/;
const SOURCE = /^(?:[A-Za-z0-9_-]{1,128}|provider:[a-f0-9]{64})$/;
class ReaderFailure extends Error {
  constructor(code = "unavailable") {
    const messages = {
      unsupported_host:
        "MDReader v1 supports local desktop VS Code only; remote and web hosts are unsupported.",
      configuration:
        "Set an absolute MDReader data directory and explicitly select 1–32 source IDs in User Settings.",
      source_not_allowed: "This source is outside the selected MDReader scope.",
      invalid_path: "Use a source-relative document path without traversal.",
      invalid_request: "Invalid or oversized reader request.",
      invalid_response: "Reader response could not be verified.",
      stale_revision:
        "The document changed. Search or reopen it to retrieve current revision evidence.",
      source_unavailable: "The selected source is unavailable.",
      cancelled: "Reader request cancelled.",
      unavailable:
        "The local MDReader service is unavailable. Start the desktop reader service and retry.",
    };
    super(messages[code] || messages.unavailable);
    this.name = "ReaderFailure";
    this.code = Object.hasOwn(messages, code) ? code : "unavailable";
  }
}
function fail(code) {
  throw new ReaderFailure(code);
}
function ensureLocalHost({ uiKind, remoteName }) {
  if (uiKind !== 1 || remoteName) fail("unsupported_host");
}
function validateDocumentPath(value) {
  if (
    typeof value !== "string" ||
    !value ||
    Buffer.byteLength(value) > 4096 ||
    /[\\\0:]/u.test(value) ||
    value.startsWith("/") ||
    value.split("/").some((c) => !c || c === "." || c === "..")
  )
    fail("invalid_path");
  return value;
}
function validateSelection(value, requireRevision = false) {
  if (
    !value ||
    typeof value !== "object" ||
    !SOURCE.test(value.source_id) ||
    typeof value.source_id !== "string"
  )
    fail("invalid_request");
  validateDocumentPath(value.path);
  if (
    (requireRevision || value.revision !== undefined) &&
    !REVISION.test(value.revision)
  )
    fail("invalid_request");
  const line = value.line ?? value.line_start ?? 1;
  if (!Number.isSafeInteger(line) || line < 1 || line > 2147483647)
    fail("invalid_request");
  return {
    source_id: value.source_id,
    path: value.path,
    ...(value.revision !== undefined ? { revision: value.revision } : {}),
    line,
  };
}
function selectionUri(value) {
  const selected = validateSelection(value, true);
  // Keep opaque source identity in the path, never in a host/port authority.
  return `mdreader://document/${encodeURIComponent(selected.source_id)}/${selected.path.split("/").map(encodeURIComponent).join("/")}?revision=${selected.revision}#line=${selected.line}`;
}
function parseSelectionUri(value) {
  if (typeof value !== "string" || value.length > 16384) fail("invalid_path");
  const modern = /^mdreader:\/\/document\/([^/?#]+)\/([^?#]+)\?([^#]+)#line=([1-9][0-9]*)$/u.exec(value);
  const match = modern ?? /^mdreader:\/\/([A-Za-z0-9_-]{1,128})\/([^?#]+)\?([^#]+)#line=([1-9][0-9]*)$/u.exec(value);
  if (!match) fail("invalid_path");
  let relative, source;
  try {
    source = modern ? decodeURIComponent(match[1]) : match[1];
    relative = match[2]
      .split("/")
      .map((part) => {
        const decoded = decodeURIComponent(part);
        if (decoded.includes("/")) fail("invalid_path");
        return decoded;
      })
      .join("/");
  } catch {
    fail("invalid_path");
  }
  const query = new URLSearchParams(match[3]);
  if ([...query.keys()].length !== 1 || !query.has("revision"))
    fail("invalid_path");
  return validateSelection(
    {
      source_id: source,
      path: relative,
      revision: query.get("revision"),
      line: Number(match[4]),
    },
    true,
  );
}
function object(value) {
  if (!value || typeof value !== "object" || Array.isArray(value))
    fail("invalid_response");
  return value;
}
function array(value) {
  if (!Array.isArray(value)) fail("invalid_response");
  return value;
}
function text(value, max = 2 * 1024 * 1024) {
  if (typeof value !== "string" || Buffer.byteLength(value) > max)
    fail("invalid_response");
  return value;
}
function integer(value) {
  if (!Number.isSafeInteger(value) || value < 0) fail("invalid_response");
  return value;
}
function reference(value, allowed) {
  object(value);
  let selected;
  try {
    selected = validateSelection(value, true);
  } catch {
    fail("invalid_response");
  }
  if (!allowed.includes(selected.source_id)) fail("invalid_response");
  return selected;
}
class ReaderClient {
  constructor({ dataDirectory, sourceIds, timeoutMs = 120000 }) {
    if (
      typeof dataDirectory !== "string" ||
      !path.isAbsolute(dataDirectory) ||
      !Array.isArray(sourceIds) ||
      sourceIds.length < 1 ||
      sourceIds.length > 32 ||
      sourceIds.some((s) => typeof s !== "string" || !SOURCE.test(s)) ||
      new Set(sourceIds).size !== sourceIds.length
    )
      fail("configuration");
    this.dataDirectory = dataDirectory;
    this.sources = Object.freeze([...sourceIds]);
    this.timeoutMs = Math.min(Math.max(timeoutMs, 10), 120000);
  }
  scope(sources = this.sources) {
    if (
      !Array.isArray(sources) ||
      !sources.length ||
      sources.length > 32 ||
      new Set(sources).size !== sources.length
    )
      fail("invalid_request");
    if (sources.some((s) => !this.sources.includes(s)))
      fail("source_not_allowed");
    return [...sources];
  }
  selected(value) {
    const selected = validateSelection(value);
    this.scope([selected.source_id]);
    return selected;
  }
  async #request(request, signal) {
    if (signal?.aborted) fail("cancelled");
    let endpoint;
    try {
      const file = await fs.open(
        path.join(this.dataDirectory, "reader-endpoint.json"),
        "r",
      );
      let bytes;
      try {
        const buffer = Buffer.alloc(4097);
        const { bytesRead } = await file.read(buffer, 0, buffer.length, 0);
        bytes = buffer.subarray(0, bytesRead);
      } finally {
        await file.close();
      }
      if (bytes.length > 4096) fail("unavailable");
      endpoint = JSON.parse(
        new TextDecoder("utf-8", { fatal: true }).decode(bytes),
      );
      if (
        endpoint.protocol !== 1 ||
        !Number.isInteger(endpoint.port) ||
        endpoint.port < 1 ||
        endpoint.port > 65535 ||
        typeof endpoint.token !== "string" ||
        !endpoint.token ||
        endpoint.token.length > 256
      )
        fail("unavailable");
    } catch {
      fail("unavailable");
    }
    if (signal?.aborted) fail("cancelled");
    const frame = Buffer.from(
      JSON.stringify({ protocol: 1, token: endpoint.token, request }) + "\n",
    );
    if (frame.length > MAX_REQUEST) fail("invalid_request");
    return new Promise((resolve, reject) => {
      const socket = net.createConnection({
        host: "127.0.0.1",
        port: endpoint.port,
      });
      const chunks = [];
      let size = 0;
      let settled = false;
      const finish = (error, value) => {
        if (settled) return;
        settled = true;
        clearTimeout(timer);
        signal?.removeEventListener("abort", abort);
        socket.destroy();
        if (error) reject(error);
        else resolve(value);
      };
      const abort = () => finish(new ReaderFailure("cancelled"));
      const timer = setTimeout(
        () => finish(new ReaderFailure("unavailable")),
        this.timeoutMs,
      );
      signal?.addEventListener("abort", abort, { once: true });
      if (signal?.aborted) {
        abort();
        return;
      }
      socket.on("connect", () => socket.end(frame));
      socket.on("error", () => finish(new ReaderFailure("unavailable")));
      socket.on("end", () => {
        if (!settled) finish(new ReaderFailure("invalid_response"));
      });
      socket.on("data", (chunk) => {
        if (settled) return;
        size += chunk.length;
        if (size > MAX_RESPONSE) {
          finish(new ReaderFailure("invalid_response"));
          return;
        }
        chunks.push(chunk);
        const newline = chunk.indexOf(10);
        if (newline < 0) return;
        try {
          const bytes = Buffer.concat(chunks);
          const end = bytes.indexOf(10);
          if (end !== bytes.length - 1) fail("invalid_response");
          const reply = object(
            JSON.parse(
              new TextDecoder("utf-8", { fatal: true }).decode(
                bytes.subarray(0, end),
              ),
            ),
          );
          if (!Object.hasOwn(reply, "result") || !Object.hasOwn(reply, "error"))
            fail("invalid_response");
          if (reply.error !== null) {
            if (reply.result !== null) fail("invalid_response");
            const code = object(reply.error).code;
            finish(
              new ReaderFailure(
                ["stale_revision", "source_unavailable", "cancelled"].includes(
                  code,
                )
                  ? code
                  : "unavailable",
              ),
            );
            return;
          }
          finish(null, reply.result);
        } catch (error) {
          finish(
            error instanceof ReaderFailure
              ? error
              : new ReaderFailure("invalid_response"),
          );
        }
      });
    });
  }
  async listSources(signal) {
    const result = array(
      await this.#request({ method: "list_sources" }, signal),
    );
    return result
      .filter((s) => s?.enabled === true && this.sources.includes(s.id))
      .map((s) => ({ id: s.id, name: text(s.name, 256), enabled: true }));
  }
  async status(sources, signal) {
    const result = object(
      await this.#request(
        { method: "status", params: { sources: this.scope(sources) } },
        signal,
      ),
    );
    return {
      documents: integer(result.documents),
      chunks: integer(result.chunks),
      embedded: integer(result.embedded),
      model: text(result.model, 256),
    };
  }
  async search(query, sources, signal) {
    if (
      typeof query !== "string" ||
      !query.trim() ||
      Buffer.byteLength(query) > 1024
    )
      fail("invalid_request");
    const scope = this.scope(sources);
    const values = array(
      await this.#request(
        { method: "search", params: { query, sources: scope, limit: 20 } },
        signal,
      ),
    );
    if (values.length > 20) fail("invalid_response");
    return values.map((value) => {
      const selected = reference(value, scope);
      if (
        !Number.isSafeInteger(value.line_start) ||
        value.line_start < 1 ||
        !Number.isSafeInteger(value.line_end) ||
        value.line_end < value.line_start ||
        !Number.isFinite(value.score)
      )
        fail("invalid_response");
      return {
        ...selected,
        title: text(value.title, 4096),
        heading: text(value.heading, 4096),
        line_start: selected.line,
        line_end: value.line_end,
        text: text(value.text),
        score: value.score,
      };
    });
  }
  async read(value, signal) {
    const selected = this.selected(value);
    const result = object(
      await this.#request(
        {
          method: "read",
          params: {
            source_id: selected.source_id,
            path: selected.path,
            revision: selected.revision ?? null,
          },
        },
        signal,
      ),
    );
    const verified = reference(result, [selected.source_id]);
    if (
      verified.path !== selected.path ||
      (selected.revision && verified.revision !== selected.revision)
    )
      fail("invalid_response");
    return {
      ...verified,
      line: selected.line,
      markdown: text(result.markdown),
    };
  }
  async related(value, signal) {
    const selected = this.selected(value);
    const results = array(
      await this.#request(
        {
          method: "related",
          params: {
            source_id: selected.source_id,
            path: selected.path,
            limit: 100,
          },
        },
        signal,
      ),
    );
    if (results.length > 100) fail("invalid_response");
    return results.map((value) => reference(value, [selected.source_id]));
  }
  async resolve(value, target, signal) {
    const selected = this.selected(value);
    if (
      typeof target !== "string" ||
      Buffer.byteLength(target) > 4096 ||
      /[\\\0:]/u.test(target)
    )
      fail("invalid_request");
    const results = array(
      await this.#request(
        {
          method: "resolve",
          params: {
            source_id: selected.source_id,
            from: selected.path,
            target,
          },
        },
        signal,
      ),
    );
    return results
      .slice(0, 100)
      .map((value) => reference(value, [selected.source_id]));
  }
}
module.exports = {
  ReaderClient,
  ReaderFailure,
  ensureLocalHost,
  validateDocumentPath,
  validateSelection,
  selectionUri,
  parseSelectionUri,
};
