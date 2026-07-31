import { describe, expect, it, vi } from 'vitest';

import { createCatdbClient } from './client.js';
import { CatdbApiError } from './errors.js';
import type {
  ApplyRequest,
  ApplyResponse,
  BuildContextRequest,
  BuildContextResponse,
  EntityQueryRequest,
  EntityQueryResponse,
  ReadRequest,
  ReadResponse,
  DocumentsResponse,
  RelatedRequest,
  RelatedResponse,
  SearchRequest,
  SearchResponse,
  SourcesResponse,
} from './types.js';

const BASE = 'http://catdb.test';

/** A captured `fetch` call. */
interface Capture {
  url: string;
  method: string;
  headers: Headers;
  body: unknown;
  signal: AbortSignal | null;
}

/**
 * Build a `fetch` double that returns `responseBody` as JSON (with `status`)
 * and records the request it received. No server is involved.
 */
function mockFetch(responseBody: unknown, status = 200): { fetch: typeof fetch; last: () => Capture } {
  const calls: Capture[] = [];
  const fetchMock = vi.fn(
    async (input: Parameters<typeof fetch>[0], init?: Parameters<typeof fetch>[1]) => {
      const headers = new Headers(init?.headers);
      calls.push({
        url: String(input),
        method: init?.method ?? 'GET',
        headers,
        body: typeof init?.body === 'string' ? JSON.parse(init.body) : init?.body,
        signal: init?.signal ?? null,
      });
      return new Response(JSON.stringify(responseBody), {
        status,
        headers: { 'content-type': 'application/json' },
      });
    },
  ) as unknown as typeof fetch;
  return {
    fetch: fetchMock,
    last: () => {
      const call = calls.at(-1);
      if (!call) throw new Error('fetch was never called');
      return call;
    },
  };
}

describe('createCatdbClient — typed request/response round-trips', () => {
  it('search: POSTs /v1/search with the JSON body and returns SearchResponse', async () => {
    const response: SearchResponse = { count: 0, hits: [] };
    const { fetch, last } = mockFetch(response);
    const client = createCatdbClient({ baseUrl: BASE, fetch });

    const request: SearchRequest = { q: 'storage tag:decision', limit: 5, explain: true };
    const result = await client.search(request);

    const call = last();
    expect(call.method).toBe('POST');
    expect(call.url).toBe(`${BASE}/v1/search`);
    expect(call.headers.get('content-type')).toBe('application/json');
    expect(call.body).toEqual(request);
    // The result is typed as SearchResponse; `count` is a number by construction.
    expect(result).toEqual(response);
    expect(result.count).toBe(0);
  });

  it('read: POSTs /v1/read and returns a typed ReadResponse', async () => {
    const response: ReadResponse = {
      content: '# Title\n\nbody',
      document: { id: 'doc-1', file_path: 'docs/a.md', content_hash: 'sha256:abc' },
      sections: [],
    };
    const { fetch, last } = mockFetch(response);
    const client = createCatdbClient({ baseUrl: BASE, fetch });

    const request: ReadRequest = { reference: 'docs/a.md' };
    const result = await client.read(request);

    expect(last().url).toBe(`${BASE}/v1/read`);
    expect(last().body).toEqual(request);
    expect(result.document.file_path).toBe('docs/a.md');
  });

  it('related: POSTs /v1/related and returns nested RelatedDocDto items', async () => {
    const response: RelatedResponse = {
      document: { id: 'doc-1', file_path: 'docs/a.md', content_hash: 'sha256:abc' },
      related: [
        {
          document_id: 'doc-2',
          file_path: 'docs/b.md',
          score: 0.87,
          components: { concept: 0.3, graph: 0.4, vector: 0.17 },
        },
      ],
    };
    const { fetch, last } = mockFetch(response);
    const client = createCatdbClient({ baseUrl: BASE, fetch });

    const request: RelatedRequest = { reference: 'docs/a.md', limit: 10 };
    const result = await client.related(request);

    expect(last().url).toBe(`${BASE}/v1/related`);
    expect(last().body).toEqual(request);
    expect(result.related[0]?.components.graph).toBe(0.4);
  });

  it('sources and documents use the shared enumeration endpoints', async () => {
    const sources: SourcesResponse = {
      count: 1,
      sources: [{ id: 'source-1', document_count: 2 }],
    };
    const sourceFetch = mockFetch(sources);
    const sourceClient = createCatdbClient({ baseUrl: BASE, fetch: sourceFetch.fetch });
    await expect(sourceClient.sources()).resolves.toEqual(sources);
    expect(sourceFetch.last().method).toBe('GET');
    expect(sourceFetch.last().url).toBe(`${BASE}/v1/sources`);

    const documents: DocumentsResponse = {
      count: 1,
      documents: [
        { id: 'doc-1', file_path: 'docs/a.md', content_hash: 'sha256:abc' },
      ],
    };
    const docFetch = mockFetch(documents);
    const docClient = createCatdbClient({ baseUrl: BASE, fetch: docFetch.fetch });
    await expect(docClient.documents()).resolves.toEqual(documents);
    expect(docFetch.last().method).toBe('GET');
    expect(docFetch.last().url).toBe(`${BASE}/v1/documents`);
  });

  it('apply: POSTs /v1/apply with a tagged operation batch and returns ApplyResponse', async () => {
    const response: ApplyResponse = {
      applied_ops: ['op-1'],
      head_digest: 'sha256:head',
      op_count: 1,
    };
    const { fetch, last } = mockFetch(response);
    const client = createCatdbClient({ baseUrl: BASE, fetch });

    const request: ApplyRequest = {
      actor: 'tester',
      operations: [
        {
          type: 'assert_object',
          entity: 'notes.Note',
          object_id: '11111111-1111-1111-1111-111111111111',
          value: { title: 'hi' },
        },
      ],
    };
    const result = await client.apply(request);

    expect(last().method).toBe('POST');
    expect(last().url).toBe(`${BASE}/v1/apply`);
    expect(last().body).toEqual(request);
    expect(result.op_count).toBe(1);
    expect(result.applied_ops).toEqual(['op-1']);
  });

  it('buildContext: POSTs /v1/build_context and returns token accounting', async () => {
    const response: BuildContextResponse = {
      items: [],
      dropped: [],
      budget: { estimate: 'exact', token_budget: 2000, tokens_used: 0 },
      reason: null,
    };
    const { fetch, last } = mockFetch(response);
    const client = createCatdbClient({ baseUrl: BASE, fetch });

    const request: BuildContextRequest = { query: 'how does apply work', token_budget: 2000 };
    const result = await client.buildContext(request);

    expect(last().url).toBe(`${BASE}/v1/build_context`);
    expect(last().body).toEqual(request);
    expect(result.budget.token_budget).toBe(2000);
  });

  it('indexStatus: issues a GET with no body', async () => {
    const { fetch, last } = mockFetch({ job: null });
    const client = createCatdbClient({ baseUrl: BASE, fetch });

    await client.indexStatus();

    const call = last();
    expect(call.method).toBe('GET');
    expect(call.url).toBe(`${BASE}/v1/index_status`);
    expect(call.body).toBeUndefined();
    expect(call.headers.get('content-type')).toBeNull();
  });

  it('entityQuery and entityExplain use the version-pinned semantic contract', async () => {
    const response: EntityQueryResponse = {
      query_id: '00000000-0000-0000-0000-000000000040',
      pins: {
        catalog_resolver_id: '00000000-0000-0000-0000-000000000041',
        catalog_revision_id: '00000000-0000-0000-0000-000000000042',
        schema_version_id: '00000000-0000-0000-0000-000000000043',
        physical_schema_version_ids: [],
        mapping_version_ids: [],
        source_version_ids: [],
        policy_version_ids: [],
        provenance_rule_version_id: '00000000-0000-0000-0000-000000000044',
      },
      rows: [],
      sources: [],
      completeness: { state: 'complete' },
    };
    const { fetch, last } = mockFetch(response);
    const client = createCatdbClient({ baseUrl: BASE, fetch });
    const request: EntityQueryRequest = {
      workspace_id: '00000000-0000-0000-0000-000000000050',
      entity: 'doc.Document',
      select: ['title'],
      filters: [],
      order_by: [],
      page_size: 25,
      limit: 100,
    };

    await expect(client.entityQuery(request)).resolves.toEqual(response);
    expect(last().url).toBe(`${BASE}/v1/entity/query`);
    expect(last().body).toEqual(request);

    await expect(client.entityExplain(request)).resolves.toEqual(response);
    expect(last().url).toBe(`${BASE}/v1/entity/explain`);
    expect(last().body).toEqual(request);
  });

  it('control-plane methods use generated paths and propagate cancellation', async () => {
    const workspace = '00000000-0000-0000-0000-000000000060';
    const controller = new AbortController();
    const response = {
      workspace_id: workspace,
      resolver_id: '00000000-0000-0000-0000-000000000061',
      revision: {
        revision_id: '00000000-0000-0000-0000-000000000062',
        actor: 'fixture',
        timestamp_ms: 1,
      },
      default_schema_version_id: '00000000-0000-0000-0000-000000000063',
      schemas: [],
      mappings: [],
      sources: [],
      policies: [],
    };
    const { fetch, last } = mockFetch(response);
    const client = createCatdbClient({ baseUrl: BASE, fetch });

    await expect(
      client.controlCatalog(workspace, { signal: controller.signal }),
    ).resolves.toEqual(response);

    expect(last().url).toBe(`${BASE}/v1/control/workspaces/${workspace}/catalog`);
    expect(last().method).toBe('GET');
    expect(last().signal).toBe(controller.signal);
  });

  it('merges default headers and normalises a trailing slash in baseUrl', async () => {
    const { fetch, last } = mockFetch({ count: 0, hits: [] } satisfies SearchResponse);
    const client = createCatdbClient({
      baseUrl: `${BASE}/`,
      fetch,
      headers: { authorization: 'Bearer t0ken' },
    });

    await client.search({ q: 'x' });

    const call = last();
    expect(call.url).toBe(`${BASE}/v1/search`);
    expect(call.headers.get('authorization')).toBe('Bearer t0ken');
  });
});

// Every endpoint declares a 200 JSON body, so a 2xx that carries no body (or an
// unparseable one) is a PROTOCOL VIOLATION — surfaced loudly as a typed error
// rather than typed into every call site as a legitimate empty outcome.
describe('createCatdbClient — empty and malformed success bodies', () => {
  it('throws a typed CatdbApiError for a 204 No Content (not undefined, not SyntaxError)', async () => {
    // `new Response(null, { status: 204 })` is a genuinely bodiless response —
    // the shape a misbehaving proxy/gateway can produce.
    const fetchMock = vi.fn(
      async () => new Response(null, { status: 204 }),
    ) as unknown as typeof fetch;
    const client = createCatdbClient({ baseUrl: BASE, fetch: fetchMock });

    let caught: unknown;
    try {
      await client.search({ q: 'x' });
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(CatdbApiError);
    expect(caught).not.toBeInstanceOf(SyntaxError);
    const error = caught as CatdbApiError;
    expect(error.status).toBe(204);
    expect(error.message).toContain('empty body');
  });

  it('throws a typed CatdbApiError for a 200 with an empty body, preserving status and raw', async () => {
    const fetchMock = vi.fn(
      async () => new Response('', { status: 200, headers: { 'content-type': 'application/json' } }),
    ) as unknown as typeof fetch;
    const client = createCatdbClient({ baseUrl: BASE, fetch: fetchMock });

    let caught: unknown;
    try {
      await client.indexStatus();
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(CatdbApiError);
    expect(caught).not.toBeInstanceOf(SyntaxError);
    const error = caught as CatdbApiError;
    expect(error.status).toBe(200);
    expect(error.raw).toBe('');
    expect(error.message).toContain('empty body');
  });

  it('throws a typed CatdbApiError (not a SyntaxError) for a 2xx non-JSON body', async () => {
    const fetchMock = vi.fn(
      async () =>
        new Response('<html>gateway says hi</html>', {
          status: 200,
          headers: { 'content-type': 'text/html' },
        }),
    ) as unknown as typeof fetch;
    const client = createCatdbClient({ baseUrl: BASE, fetch: fetchMock });

    let caught: unknown;
    try {
      await client.search({ q: 'x' });
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(CatdbApiError);
    expect(caught).not.toBeInstanceOf(SyntaxError);
    const error = caught as CatdbApiError;
    expect(error.status).toBe(200);
    expect(error.raw).toBe('<html>gateway says hi</html>');
    expect(error.code).toBeUndefined();
    expect(error.message).toContain('non-JSON body');
  });
});

describe('createCatdbClient — error typing', () => {
  it('maps a 4xx ApiErrorBody to a typed CatdbApiError', async () => {
    const errorBody = {
      error: 'unknown_field',
      message: 'unknown filter field `tag`',
      details: { valid_fields: ['path', 'title'] },
    };
    const { fetch } = mockFetch(errorBody, 422);
    const client = createCatdbClient({ baseUrl: BASE, fetch });

    let caught: unknown;
    try {
      await client.search({ q: 'tag:bad' });
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(CatdbApiError);
    const error = caught as CatdbApiError;
    expect(error.status).toBe(422);
    expect(error.code).toBe('unknown_field');
    expect(error.message).toBe('unknown filter field `tag`');
    expect(error.details).toEqual({ valid_fields: ['path', 'title'] });
    expect(error.body).toEqual(errorBody);
  });

  it('decodes entity execution failures into typed source outcomes', async () => {
    const source = {
      source_id: '00000000-0000-0000-0000-000000000001',
      source_version_id: '00000000-0000-0000-0000-000000000002',
      outcome: 'rate_limited',
      retry_after_ms: 17_000,
      freshness_ms: null,
      scanned: 0,
      returned: 0,
    };
    const { fetch } = mockFetch(
      {
        error: 'entity_execution_failed',
        message: 'authorized source execution did not complete',
        details: { sources: [source] },
      },
      502,
    );
    const client = createCatdbClient({ baseUrl: BASE, fetch });

    let caught: unknown;
    try {
      await client.entityQuery({
        workspace_id: '00000000-0000-0000-0000-000000000003',
        entity: 'document.Document',
        select: ['path'],
        filters: [],
        order_by: [],
        page_size: 10,
        limit: 10,
        require_complete: true,
      });
    } catch (error) {
      caught = error;
    }

    expect(caught).toBeInstanceOf(CatdbApiError);
    expect((caught as CatdbApiError).execution).toEqual({ sources: [source] });
  });

  it('does not type malformed execution details as trusted provider evidence', async () => {
    const { fetch } = mockFetch(
      {
        error: 'entity_execution_failed',
        message: 'authorized source execution did not complete',
        details: {
          sources: [
            {
              source_id: 'source',
              source_version_id: 'version',
              outcome: 'rate_limited',
              retry_after_ms: -1,
              freshness_ms: null,
              scanned: 0,
              returned: 0,
            },
          ],
        },
      },
      502,
    );
    const client = createCatdbClient({ baseUrl: BASE, fetch });

    let caught: unknown;
    try {
      await client.entityQuery({
        workspace_id: 'workspace',
        entity: 'document.Document',
        select: ['path'],
        filters: [],
        order_by: [],
        page_size: 10,
        limit: 10,
        require_complete: true,
      });
    } catch (error) {
      caught = error;
    }

    expect(caught).toBeInstanceOf(CatdbApiError);
    expect((caught as CatdbApiError).execution).toBeUndefined();
  });

  it('falls back to a status-only message for a non-JSON error body', async () => {
    const fetchMock = vi.fn(
      async () => new Response('upstream exploded', { status: 500 }),
    ) as unknown as typeof fetch;
    const client = createCatdbClient({ baseUrl: BASE, fetch: fetchMock });

    let caught: unknown;
    try {
      await client.read({ reference: 'x' });
    } catch (e) {
      caught = e;
    }
    expect(caught).toBeInstanceOf(CatdbApiError);
    const error = caught as CatdbApiError;
    expect(error.status).toBe(500);
    expect(error.code).toBeUndefined();
    expect(error.body).toBeUndefined();
    expect(error.raw).toBe('upstream exploded');
    expect(error.message).toContain('HTTP 500');
  });
});

describe('createCatdbClient — default fetch binding', () => {
  /**
   * Browsers implement `fetch` as a method of the global object and reject a
   * detached call with `TypeError: Illegal invocation`. Storing the default as
   * `globalThis.fetch` and later invoking it as `this.#fetch(...)` re-binds the
   * receiver to the client instance, so the SDK must capture it BOUND. Without
   * that, `createCatdbClient({baseUrl})` is unusable in a browser — the SDK's
   * primary consumer — while still passing in Node, where fetch is receiver-
   * agnostic. This test models the browser rule explicitly.
   */
  it('calls the default global fetch with the global as its receiver', async () => {
    const original = globalThis.fetch;
    const seen: unknown[] = [];
    const browserLikeFetch = function (this: unknown) {
      // Mirror the browser: refuse a detached receiver.
      if (this !== globalThis && this !== undefined) {
        throw new TypeError("Illegal invocation");
      }
      seen.push(this);
      return Promise.resolve(
        new Response(JSON.stringify({ hits: [] }), {
          status: 200,
          headers: { 'content-type': 'application/json' },
        }),
      );
    } as unknown as typeof fetch;

    globalThis.fetch = browserLikeFetch;
    try {
      // No `fetch` in options — exercise the default capture path.
      const client = createCatdbClient({ baseUrl: BASE });
      const result = await client.search({ q: 'anything' });
      expect(result).toEqual({ hits: [] });
      expect(seen).toHaveLength(1);
    } finally {
      globalThis.fetch = original;
    }
  });
});
