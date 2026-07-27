import { afterEach, describe, expect, test, vi } from 'vitest';
import type {
  EntityQueryResponse,
  SessionResponse,
} from '@catdb/client';

import {
  createEnterpriseMobileSourceAdapter,
  MobileSourceReadError,
  type MobileLibraryDocument,
} from './mobile-source-adapter';

const config = {
  connectionId: 'managed-connection-42',
};

const IDS = {
  deployment: '00000000-0000-0000-0000-000000000001',
  workspace: '00000000-0000-0000-0000-000000000002',
  resolver: '00000000-0000-0000-0000-000000000003',
  catalog: '00000000-0000-0000-0000-000000000004',
  schema: '00000000-0000-0000-0000-000000000005',
  source: '00000000-0000-0000-0000-000000000006',
  sourceVersion: '00000000-0000-0000-0000-000000000007',
  mapping: '00000000-0000-0000-0000-000000000008',
  policy: '00000000-0000-0000-0000-000000000009',
  query: '00000000-0000-0000-0000-000000000010',
} as const;

const session = {
  deployment_id: IDS.deployment,
  principal_kind: 'oidc',
  scopes: ['query'],
  workspace_ids: [IDS.workspace],
} satisfies SessionResponse;

function observation(value: unknown, observedAt = 1_234) {
  return {
    source_version_id: IDS.sourceVersion,
    record_key: 'document-1',
    mapping_version_id: IDS.mapping,
    observed_at_ms: observedAt,
    state:
      value === null
        ? ({ state: 'observed_null' as const })
        : ({ state: 'observed_value' as const, value }),
  };
}

function queryResponse(
  fields: Record<string, unknown>,
  overrides: Partial<EntityQueryResponse> = {},
): EntityQueryResponse {
  return {
    query_id: IDS.query,
    pins: {
      catalog_resolver_id: IDS.resolver,
      catalog_revision_id: IDS.catalog,
      schema_version_id: IDS.schema,
      physical_schema_version_ids: [],
      mapping_version_ids: [IDS.mapping],
      source_version_ids: [IDS.sourceVersion],
      policy_version_ids: [IDS.policy],
    },
    rows: [
      {
        record_key: 'document-1',
        fields,
        provenance: Object.fromEntries(
          Object.entries(fields).map(([field, value]) => [
            field,
            {
              field,
              selected: 0,
              considered: [observation(value)],
            },
          ]),
        ),
      },
    ],
    sources: [
      {
        source_id: IDS.source,
        source_version_id: IDS.sourceVersion,
        scanned: 1,
        returned: 1,
        freshness_ms: 1,
        outcome: 'ran',
      },
    ],
    completeness: { state: 'complete' },
    ...overrides,
  };
}

function response(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

function sequence(...responses: Response[]) {
  return vi.fn(async (_input: RequestInfo | URL, _init?: RequestInit) => {
    void _input;
    void _init;
    const next = responses.shift();
    if (!next) throw new Error('unexpected request');
    return next;
  });
}

afterEach(() => {
  vi.restoreAllMocks();
});

describe('authenticated read-only Enterprise mobile source adapter', () => {
  test('uses the generated session/entity contract and carries pinned identities', async () => {
    const fetcher = sequence(
      response(session),
      response(
        queryResponse({
          path: 'papers/architecture.md',
          title: 'Semantic architecture',
          size: 128,
        }),
      ),
    );
    const adapter = createEnterpriseMobileSourceAdapter(config, fetcher);

    await expect(adapter.listDocuments()).resolves.toEqual({
      status: 'ready',
      label: 'Enterprise Document library',
      documents: [
        {
          source: { kind: 'enterprise-catdb', id: IDS.source },
          sourceRoot: { id: IDS.workspace, version: IDS.catalog },
          document: { id: 'document-1', path: 'papers/architecture.md' },
          name: 'architecture.md',
          title: 'Semantic architecture',
          size: 128,
        },
      ],
    });

    expect(fetcher).toHaveBeenNthCalledWith(
      1,
      'http://mdreader-native.invalid/v1/session',
      expect.objectContaining({ method: 'GET' }),
    );
    const firstHeaders = new Headers(fetcher.mock.calls[0]?.[1]?.headers);
    expect(firstHeaders.has('authorization')).toBe(false);
    expect(firstHeaders.has('x-mdreader-session-reference')).toBe(false);

    expect(fetcher).toHaveBeenNthCalledWith(
      2,
      'http://mdreader-native.invalid/v1/entity/query',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          workspace_id: IDS.workspace,
          entity: 'document.Document',
          select: ['path', 'title', 'size'],
          filters: [],
          order_by: [{ field: 'path', direction: 'ascending' }],
          page_size: 1000,
          limit: 1000,
          require_complete: true,
        }),
      }),
    );
  });

  test('keeps authorization refusal, transport unavailability, and malformed answers distinct', async () => {
    const refused = createEnterpriseMobileSourceAdapter(
      config,
      vi.fn(async () => response({ error: 'forbidden', message: 'denied' }, 403)),
    );
    await expect(refused.listDocuments()).resolves.toEqual({
      status: 'refused',
      message: 'The Enterprise session was refused.',
    });

    const unavailable = createEnterpriseMobileSourceAdapter(
      config,
      vi.fn(async () => {
        throw new TypeError('offline');
      }),
    );
    await expect(unavailable.listDocuments()).resolves.toEqual({
      status: 'unavailable',
      message: 'The Enterprise server is unavailable.',
    });

    const malformed = createEnterpriseMobileSourceAdapter(
      config,
      sequence(response(session), response({ rows: [] })),
    );
    await expect(malformed.listDocuments()).resolves.toEqual({
      status: 'invalid-response',
      message: 'The Enterprise server returned an invalid semantic response.',
    });
  });

  test('rejects an oversized response before parsing it', async () => {
    const oversized = createEnterpriseMobileSourceAdapter(
      config,
      vi.fn(async () =>
        new Response('{}', {
          status: 200,
          headers: { 'content-length': String(4 * 1024 * 1024) },
        }),
      ),
    );

    await expect(oversized.listDocuments()).resolves.toEqual({
      status: 'invalid-response',
      message: 'The Enterprise server response exceeded the mobile safety bound.',
    });
  });

  test('refuses a session without query scope or one exact workspace', async () => {
    const noQuery = createEnterpriseMobileSourceAdapter(
      config,
      sequence(response({ ...session, scopes: ['catalog_read'] })),
    );
    await expect(noQuery.listDocuments()).resolves.toEqual({
      status: 'refused',
      message: 'The Enterprise session does not allow document queries.',
    });

    const ambiguous = createEnterpriseMobileSourceAdapter(
      config,
      sequence(response({ ...session, workspace_ids: [IDS.workspace, IDS.deployment] })),
    );
    await expect(ambiguous.listDocuments()).resolves.toEqual({
      status: 'invalid-response',
      message: 'Choose an Enterprise session restricted to one workspace.',
    });
  });

  test('document reads pin the catalog and source and retain field provenance', async () => {
    const listing = queryResponse({
      path: 'papers/architecture.md',
      title: 'Semantic architecture',
      size: 128,
    });
    const read = queryResponse({
      path: 'papers/architecture.md',
      title: 'Semantic architecture',
      size: 128,
      body: '# Semantic architecture',
      modifiedAt: 1_234,
    });
    const fetcher = sequence(response(session), response(listing), response(read));
    const adapter = createEnterpriseMobileSourceAdapter(config, fetcher);
    const listed = await adapter.listDocuments();
    if (listed.status !== 'ready') throw new Error('fixture listing was not ready');
    const document = listed.documents[0]!;

    await expect(adapter.readDocument(document)).resolves.toEqual(
      expect.objectContaining({
        source: document.source,
        sourceRoot: document.sourceRoot,
        document: document.document,
        content: '# Semantic architecture',
        observedAt: 1_234,
        provenance: {
          pins: read.pins,
          fields: read.rows[0]!.provenance,
          sources: read.sources,
        },
      }),
    );
    expect(fetcher).toHaveBeenLastCalledWith(
      'http://mdreader-native.invalid/v1/entity/query',
      expect.objectContaining({
        method: 'POST',
        body: JSON.stringify({
          workspace_id: IDS.workspace,
          catalog_revision_id: IDS.catalog,
          entity: 'document.Document',
          select: ['path', 'title', 'size', 'body'],
          filters: [{ field: 'path', operator: 'eq', value: 'papers/architecture.md' }],
          order_by: [],
          source_ids: [IDS.source],
          page_size: 2,
          limit: 2,
          require_complete: true,
        }),
      }),
    );
  });

  test('preserves a titleless Markdown document and an empty body as valid data', async () => {
    const listing = queryResponse({
      path: 'papers/untitled.md',
      title: null,
      size: 0,
    });
    const read = queryResponse({
      path: 'papers/untitled.md',
      title: null,
      size: 0,
      body: '',
    });
    const adapter = createEnterpriseMobileSourceAdapter(
      config,
      sequence(response(session), response(listing), response(read)),
    );

    const listed = await adapter.listDocuments();
    expect(listed).toMatchObject({
      status: 'ready',
      documents: [{ name: 'untitled.md', title: null, size: 0 }],
    });
    if (listed.status !== 'ready') throw new Error('fixture listing was not ready');
    await expect(adapter.readDocument(listed.documents[0]!)).resolves.toMatchObject({
      content: '',
      provenance: {
        pins: read.pins,
        fields: read.rows[0]!.provenance,
      },
    });
  });

  test('a document answer under a different source identity fails closed', async () => {
    const read = queryResponse(
      {
        path: 'papers/architecture.md',
        title: 'Semantic architecture',
        size: 128,
        body: '# Wrong source',
        modifiedAt: 1_234,
      },
      {
        sources: [
          {
            source_id: IDS.deployment,
            source_version_id: IDS.sourceVersion,
            scanned: 1,
            returned: 1,
            freshness_ms: 1,
            outcome: 'ran',
          },
        ],
      },
    );
    const adapter = createEnterpriseMobileSourceAdapter(config, sequence(response(read)));
    const document = {
      source: { kind: 'enterprise-catdb', id: IDS.source },
      sourceRoot: { id: IDS.workspace, version: IDS.catalog },
      document: { id: 'document-1', path: 'papers/architecture.md' },
      name: 'architecture.md',
      title: 'Semantic architecture',
      size: 128,
    } satisfies MobileLibraryDocument;

    await expect(adapter.readDocument(document)).rejects.toMatchObject({
      name: MobileSourceReadError.name,
      kind: 'identity-mismatch',
    });
  });
});
