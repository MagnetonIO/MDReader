import { afterEach, describe, expect, test, vi } from 'vitest';
import type {
  EntityQueryResponse,
  SessionResponse,
} from '@catdb/client';

import { commands } from '../bindings';
import {
  createEnterpriseMobileSourceAdapter,
  MobileSourceReadError,
  type MobileLibraryDocument,
} from './mobile-source-adapter';
import { mobileProvenanceOf } from './mobile-provenance';

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
  source2: '00000000-0000-0000-0000-000000000011',
  sourceVersion2: '00000000-0000-0000-0000-000000000012',
  mapping2: '00000000-0000-0000-0000-000000000013',
  physical: '00000000-0000-0000-0000-000000000014',
  physical2: '00000000-0000-0000-0000-000000000015',
  provenanceRule: '00000000-0000-0000-0000-000000000016',
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
      physical_schema_version_ids: [IDS.physical],
      mapping_version_ids: [IDS.mapping],
      source_version_ids: [IDS.sourceVersion],
      policy_version_ids: [IDS.policy],
      provenance_rule_version_id: IDS.provenanceRule,
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
              provenance_rule_version_id: IDS.provenanceRule,
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

type QueryResponseMutation = {
  label: string;
  mutate(response: EntityQueryResponse): void;
};

const invalidProvenanceRulePins: QueryResponseMutation[] = [
  {
    label: 'a missing top-level provenance rule pin',
    mutate(response) {
      delete (response.pins as unknown as Record<string, unknown>)
        .provenance_rule_version_id;
    },
  },
  {
    label: 'a missing field provenance rule pin',
    mutate(response) {
      const field = Object.values(response.rows[0]!.provenance)[0];
      if (!field) throw new Error('fixture field provenance is missing');
      delete (field as unknown as Record<string, unknown>).provenance_rule_version_id;
    },
  },
  {
    label: 'a field provenance rule pin that does not match the response pin',
    mutate(response) {
      const field = Object.values(response.rows[0]!.provenance)[0];
      if (!field) throw new Error('fixture field provenance is missing');
      field.provenance_rule_version_id = IDS.catalog;
    },
  },
];

function addSecondProvider(response: EntityQueryResponse): void {
  response.pins.physical_schema_version_ids.push(IDS.physical2);
  response.pins.mapping_version_ids.push(IDS.mapping2);
  response.pins.source_version_ids.push(IDS.sourceVersion2);
  response.sources.push({
    source_id: IDS.source2,
    source_version_id: IDS.sourceVersion2,
    scanned: 1,
    returned: 1,
    freshness_ms: 1,
    outcome: 'ran',
  });
  for (const [field, fieldProvenance] of Object.entries(response.rows[0]!.provenance)) {
    if (field === 'body') {
      fieldProvenance.considered[0] = {
        source_version_id: IDS.sourceVersion,
        record_key: 'document-1',
        mapping_version_id: IDS.mapping,
        observed_at_ms: 1_234,
        state: { state: 'field_unseen' },
      };
      fieldProvenance.considered.push({
        source_version_id: IDS.sourceVersion2,
        record_key: 'document-1',
        mapping_version_id: IDS.mapping2,
        observed_at_ms: 1_234,
        state: {
          state: 'observed_value',
          value: response.rows[0]!.fields[field],
        },
      });
      fieldProvenance.selected = 1;
    } else {
      fieldProvenance.considered.push({
        source_version_id: IDS.sourceVersion2,
        record_key: 'document-1',
        mapping_version_id: IDS.mapping2,
        observed_at_ms: 1_234,
        state: { state: 'field_unseen' },
      });
    }
  }
}

function response(body: unknown, status = 200): Response {
  return new Response(JSON.stringify(body), {
    status,
    headers: { 'content-type': 'application/json' },
  });
}

function nativeResponse(body: unknown, status = 200) {
  return {
    status: 'ok' as const,
    data: { status, body_json: JSON.stringify(body) },
  };
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
          source: {
            kind: 'enterprise-catdb',
            id: IDS.deployment,
            connectionId: config.connectionId,
            deploymentId: IDS.deployment,
          },
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

  test('listing preserves a native connection identity change with reconnect guidance', async () => {
    vi.spyOn(commands, 'mobileEnterpriseRequest').mockResolvedValue({
      status: 'error',
      error: { kind: 'connection-identity-changed' },
    });
    const adapter = createEnterpriseMobileSourceAdapter(config);

    await expect(adapter.listDocuments()).resolves.toEqual({
      status: 'identity-changed',
      message:
        'The Enterprise connection identity changed. Reconnect and verify the server and workspace before trying again.',
    });
  });

  test('reading preserves a native connection identity change with reconnect guidance', async () => {
    const listing = queryResponse({
      path: 'papers/architecture.md',
      title: 'Semantic architecture',
      size: 128,
    });
    vi.spyOn(commands, 'mobileEnterpriseRequest')
      .mockResolvedValueOnce(nativeResponse(session))
      .mockResolvedValueOnce(nativeResponse(listing))
      .mockResolvedValueOnce({
        status: 'error',
        error: { kind: 'connection-identity-changed' },
      });
    const adapter = createEnterpriseMobileSourceAdapter(config);
    const listed = await adapter.listDocuments();
    if (listed.status !== 'ready') throw new Error('fixture listing was not ready');

    await expect(adapter.readDocument(listed.documents[0]!)).rejects.toMatchObject({
      name: MobileSourceReadError.name,
      kind: 'identity-changed',
      message:
        'The Enterprise connection identity changed. Reconnect and verify the server and workspace before reading this document again.',
    });
  });

  test('preserves a typed provider retry instead of collapsing it to unavailability', async () => {
    const adapter = createEnterpriseMobileSourceAdapter(
      config,
      sequence(
        response(session),
        response(
          {
            error: 'entity_execution_failed',
            message: 'authorized source execution did not complete',
            details: {
              sources: [
                {
                  source_id: IDS.source,
                  source_version_id: IDS.sourceVersion,
                  outcome: 'rate_limited',
                  retry_after_ms: 17_000,
                  freshness_ms: null,
                  scanned: 0,
                  returned: 0,
                },
              ],
            },
          },
          502,
        ),
      ),
    );

    await expect(adapter.listDocuments()).resolves.toEqual({
      status: 'rate-limited',
      message: 'The Enterprise source asked MDReader to retry later.',
      retryAfterMs: 17_000,
    });
  });

  test('rejects failed source outcomes inside a response declared complete', async () => {
    const contradictory = queryResponse({
      path: 'papers/partial.md',
      title: 'Partial',
      size: 128,
    });
    contradictory.sources[0] = {
      ...contradictory.sources[0]!,
      outcome: 'incomplete_search',
      retry_after_ms: null,
    };
    const adapter = createEnterpriseMobileSourceAdapter(
      config,
      sequence(response(session), response(contradictory)),
    );

    await expect(adapter.listDocuments()).resolves.toEqual({
      status: 'invalid-response',
      message: 'The Enterprise server returned an invalid semantic response.',
    });
  });

  test('rejects invalid retry timing even when partial completeness matches the source', () => {
    const partial = queryResponse({
      path: 'papers/partial.md',
      title: 'Partial',
      size: 128,
    });
    partial.sources[0] = {
      ...partial.sources[0]!,
      outcome: 'rate_limited',
      retry_after_ms: -1,
    };
    partial.completeness = {
      state: 'partial',
      excluded_source_ids: [IDS.source],
    };

    expect(
      mobileProvenanceOf(
        {
          pins: partial.pins,
          fields: {},
          sources: partial.sources,
        },
        undefined,
        partial.completeness,
      ),
    ).toBeNull();
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
          page_size: 2,
          limit: 2,
          require_complete: true,
        }),
      }),
    );
  });

  test('preserves typed provider retry evidence when a document read fails', async () => {
    const listing = queryResponse({
      path: 'papers/retry.md',
      title: 'Retry',
      size: 128,
    });
    const adapter = createEnterpriseMobileSourceAdapter(
      config,
      sequence(
        response(session),
        response(listing),
        response(
          {
            error: 'entity_execution_failed',
            message: 'authorized source execution did not complete',
            details: {
              sources: [
                {
                  source_id: IDS.source,
                  source_version_id: IDS.sourceVersion,
                  outcome: 'rate_limited',
                  retry_after_ms: 3_000,
                  freshness_ms: null,
                  scanned: 0,
                  returned: 0,
                },
              ],
            },
          },
          502,
        ),
      ),
    );
    const listed = await adapter.listDocuments();
    if (listed.status !== 'ready') throw new Error('fixture listing was not ready');

    await expect(adapter.readDocument(listed.documents[0]!)).rejects.toMatchObject({
      name: MobileSourceReadError.name,
      kind: 'rate-limited',
      retryAfterMs: 3_000,
    });
  });

  test('preserves logical document identity when path and body select different providers', async () => {
    const listing = queryResponse({
      path: 'papers/federated.md',
      title: 'Federated',
      size: 19,
    });
    const read = queryResponse({
      path: 'papers/federated.md',
      title: 'Federated',
      size: 19,
      body: '# Federated content',
    });
    addSecondProvider(read);
    const fetcher = sequence(response(session), response(listing), response(read));
    const adapter = createEnterpriseMobileSourceAdapter(config, fetcher);
    const listed = await adapter.listDocuments();
    if (listed.status !== 'ready') throw new Error('fixture listing was not ready');
    expect(listed.documents[0]!.source).toEqual({
      kind: 'enterprise-catdb',
      id: IDS.deployment,
      connectionId: config.connectionId,
      deploymentId: IDS.deployment,
    });

    await expect(adapter.readDocument(listed.documents[0]!)).resolves.toMatchObject({
      content: '# Federated content',
      provenance: {
        pins: read.pins,
        fields: read.rows[0]!.provenance,
        sources: read.sources,
      },
    });
    const readBody = JSON.parse(String(fetcher.mock.calls[2]?.[1]?.body));
    expect(readBody).not.toHaveProperty('source_ids');
  });

  test('listing rejects a pinned source omitted from source reports before admitting the service', async () => {
    const listing = queryResponse({
      path: 'papers/architecture.md',
      title: 'Semantic architecture',
      size: 128,
    });
    addSecondProvider(listing);
    listing.sources.pop();
    const adapter = createEnterpriseMobileSourceAdapter(
      config,
      sequence(response(session), response(listing)),
    );

    await expect(adapter.listDocuments()).resolves.toEqual({
      status: 'invalid-response',
      message: 'The Enterprise server returned an invalid semantic response.',
    });
  });

  test('listing rejects duplicate provider observations that omit a pinned source', async () => {
    const listing = queryResponse({
      path: 'papers/architecture.md',
      title: 'Semantic architecture',
      size: 128,
    });
    addSecondProvider(listing);
    const title = listing.rows[0]!.provenance.title;
    if (!title) throw new Error('fixture title provenance is missing');
    title.considered = [
      structuredClone(title.considered[0]!),
      structuredClone(title.considered[0]!),
    ];
    const adapter = createEnterpriseMobileSourceAdapter(
      config,
      sequence(response(session), response(listing)),
    );

    await expect(adapter.listDocuments()).resolves.toEqual({
      status: 'invalid-response',
      message: 'The Enterprise server returned an invalid semantic response.',
    });
  });

  test('listing rejects two rows that share one record key', async () => {
    const listing = queryResponse({
      path: 'papers/architecture.md',
      title: 'Semantic architecture',
      size: 128,
    });
    const twin = structuredClone(listing.rows[0]!);
    twin.fields.path = 'papers/provenance.md';
    for (const [field, fieldProvenance] of Object.entries(twin.provenance)) {
      if (field === 'path') {
        for (const considered of fieldProvenance.considered) {
          considered.state = { state: 'observed_value', value: twin.fields.path };
        }
      }
    }
    listing.rows.push(twin);
    const adapter = createEnterpriseMobileSourceAdapter(
      config,
      sequence(response(session), response(listing)),
    );

    await expect(adapter.listDocuments()).resolves.toEqual({
      status: 'invalid-response',
      message: 'The Enterprise server returned an ambiguous document identity.',
    });
  });

  test('listing rejects distinct records that claim one document path', async () => {
    const listing = queryResponse({
      path: 'papers/architecture.md',
      title: 'Semantic architecture',
      size: 128,
    });
    const twin = structuredClone(listing.rows[0]!);
    twin.record_key = 'document-2';
    for (const fieldProvenance of Object.values(twin.provenance)) {
      for (const considered of fieldProvenance.considered) {
        considered.record_key = 'document-2';
      }
    }
    listing.rows.push(twin);
    const adapter = createEnterpriseMobileSourceAdapter(
      config,
      sequence(response(session), response(listing)),
    );

    await expect(adapter.listDocuments()).resolves.toEqual({
      status: 'invalid-response',
      message: 'The Enterprise server returned an ambiguous document identity.',
    });
  });

  test('listing admits distinct records at distinct document paths', async () => {
    const listing = queryResponse({
      path: 'papers/architecture.md',
      title: 'Semantic architecture',
      size: 128,
    });
    const twin = structuredClone(listing.rows[0]!);
    twin.record_key = 'document-2';
    twin.fields.path = 'papers/provenance.md';
    for (const [field, fieldProvenance] of Object.entries(twin.provenance)) {
      for (const considered of fieldProvenance.considered) {
        considered.record_key = 'document-2';
        if (field === 'path') {
          considered.state = { state: 'observed_value', value: twin.fields.path };
        }
      }
    }
    listing.rows.push(twin);
    const adapter = createEnterpriseMobileSourceAdapter(
      config,
      sequence(response(session), response(listing)),
    );

    const result = await adapter.listDocuments();
    expect(result.status).toBe('ready');
    expect(
      result.status === 'ready'
        ? result.documents.map((document) => document.document.id)
        : [],
    ).toEqual(['document-1', 'document-2']);
  });

  test('listing rejects provider observations reordered relative to pinned source order', async () => {
    const listing = queryResponse({
      path: 'papers/architecture.md',
      title: 'Semantic architecture',
      size: 128,
    });
    addSecondProvider(listing);
    const size = listing.rows[0]!.provenance.size;
    if (!size) throw new Error('fixture size provenance is missing');
    size.considered.reverse();
    size.selected = 1;
    const adapter = createEnterpriseMobileSourceAdapter(
      config,
      sequence(response(session), response(listing)),
    );

    await expect(adapter.listDocuments()).resolves.toEqual({
      status: 'invalid-response',
      message: 'The Enterprise server returned an invalid semantic response.',
    });
  });

  test('listing rejects source reports reordered relative to pinned source order', async () => {
    const listing = queryResponse({
      path: 'papers/architecture.md',
      title: 'Semantic architecture',
      size: 128,
    });
    addSecondProvider(listing);
    listing.sources.reverse();
    const adapter = createEnterpriseMobileSourceAdapter(
      config,
      sequence(response(session), response(listing)),
    );

    await expect(adapter.listDocuments()).resolves.toEqual({
      status: 'invalid-response',
      message: 'The Enterprise server returned an invalid semantic response.',
    });
  });

  test('listing rejects an unknown source outcome before admitting the service', async () => {
    const listing = queryResponse({
      path: 'papers/architecture.md',
      title: 'Semantic architecture',
      size: 128,
    });
    listing.sources[0]!.outcome =
      'invented-terminal-state' as EntityQueryResponse['sources'][number]['outcome'];
    const adapter = createEnterpriseMobileSourceAdapter(
      config,
      sequence(response(session), response(listing)),
    );

    await expect(adapter.listDocuments()).resolves.toEqual({
      status: 'invalid-response',
      message: 'The Enterprise server returned an invalid semantic response.',
    });
  });

  for (const invalidPin of invalidProvenanceRulePins) {
    test(`listing rejects ${invalidPin.label}`, async () => {
      const listing = queryResponse({
        path: 'papers/architecture.md',
        title: 'Semantic architecture',
        size: 128,
      });
      invalidPin.mutate(listing);
      const adapter = createEnterpriseMobileSourceAdapter(
        config,
        sequence(response(session), response(listing)),
      );

      await expect(adapter.listDocuments()).resolves.toEqual({
        status: 'invalid-response',
        message: 'The Enterprise server returned an invalid semantic response.',
      });
    });
  }

  test('listing retains exact provider order while different fields select different winners', async () => {
    const listing = queryResponse({
      path: 'papers/federated.md',
      title: 'Federated title',
      size: 19,
    });
    addSecondProvider(listing);
    const title = listing.rows[0]!.provenance.title;
    if (!title) throw new Error('fixture title provenance is missing');
    title.considered[0] = {
      source_version_id: IDS.sourceVersion,
      record_key: 'document-1',
      mapping_version_id: IDS.mapping,
      observed_at_ms: 1_234,
      state: { state: 'field_unseen' },
    };
    title.considered[1] = {
      source_version_id: IDS.sourceVersion2,
      record_key: 'document-1',
      mapping_version_id: IDS.mapping2,
      observed_at_ms: 1_234,
      state: { state: 'observed_value', value: 'Federated title' },
    };
    title.selected = 1;
    const adapter = createEnterpriseMobileSourceAdapter(
      config,
      sequence(response(session), response(listing)),
    );

    await expect(adapter.listDocuments()).resolves.toMatchObject({
      status: 'ready',
      documents: [
        {
          document: { id: 'document-1', path: 'papers/federated.md' },
          title: 'Federated title',
          size: 19,
        },
      ],
    });
  });

  test('rejects a pinned source that is omitted from source reports and field provenance', async () => {
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
    });
    read.pins.physical_schema_version_ids.push(IDS.physical2);
    read.pins.mapping_version_ids.push(IDS.mapping2);
    read.pins.source_version_ids.push(IDS.sourceVersion2);
    const adapter = createEnterpriseMobileSourceAdapter(
      config,
      sequence(response(session), response(listing), response(read)),
    );
    const listed = await adapter.listDocuments();
    if (listed.status !== 'ready') throw new Error('fixture listing was not ready');

    await expect(adapter.readDocument(listed.documents[0]!)).rejects.toMatchObject({
      name: MobileSourceReadError.name,
      kind: 'invalid-response',
    });
  });

  test('rejects duplicate provider observations that omit another reported source', async () => {
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
    });
    addSecondProvider(read);
    const bodyProvenance = read.rows[0]!.provenance.body;
    if (!bodyProvenance) throw new Error('fixture body provenance is missing');
    const selected = structuredClone(bodyProvenance.considered[1]!);
    bodyProvenance.considered = [selected, structuredClone(selected)];
    bodyProvenance.selected = 0;
    const adapter = createEnterpriseMobileSourceAdapter(
      config,
      sequence(response(session), response(listing), response(read)),
    );
    const listed = await adapter.listDocuments();
    if (listed.status !== 'ready') throw new Error('fixture listing was not ready');

    await expect(adapter.readDocument(listed.documents[0]!)).rejects.toMatchObject({
      name: MobileSourceReadError.name,
      kind: 'invalid-response',
    });
  });

  test('rejects field observations reordered relative to pinned source order', async () => {
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
    });
    addSecondProvider(read);
    const bodyProvenance = read.rows[0]!.provenance.body;
    if (!bodyProvenance) throw new Error('fixture body provenance is missing');
    bodyProvenance.considered.reverse();
    bodyProvenance.selected = 0;
    const adapter = createEnterpriseMobileSourceAdapter(
      config,
      sequence(response(session), response(listing), response(read)),
    );
    const listed = await adapter.listDocuments();
    if (listed.status !== 'ready') throw new Error('fixture listing was not ready');

    await expect(adapter.readDocument(listed.documents[0]!)).rejects.toMatchObject({
      name: MobileSourceReadError.name,
      kind: 'invalid-response',
    });
  });

  test('rejects source reports reordered relative to pinned source order', async () => {
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
    });
    addSecondProvider(read);
    read.sources.reverse();
    const adapter = createEnterpriseMobileSourceAdapter(
      config,
      sequence(response(session), response(listing), response(read)),
    );
    const listed = await adapter.listDocuments();
    if (listed.status !== 'ready') throw new Error('fixture listing was not ready');

    await expect(adapter.readDocument(listed.documents[0]!)).rejects.toMatchObject({
      name: MobileSourceReadError.name,
      kind: 'invalid-response',
    });
  });

  for (const invalidPin of invalidProvenanceRulePins) {
    test(`document reads reject ${invalidPin.label}`, async () => {
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
      invalidPin.mutate(read);
      const adapter = createEnterpriseMobileSourceAdapter(
        config,
        sequence(response(session), response(listing), response(read)),
      );
      const listed = await adapter.listDocuments();
      if (listed.status !== 'ready') throw new Error('fixture listing was not ready');

      await expect(adapter.readDocument(listed.documents[0]!)).rejects.toMatchObject({
        name: MobileSourceReadError.name,
        kind: 'invalid-response',
      });
    });
  }

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

  test('a document under a different admitted CatDB service identity fails closed', async () => {
    const listing = queryResponse({
      path: 'papers/architecture.md',
      title: 'Semantic architecture',
      size: 128,
    });
    const adapter = createEnterpriseMobileSourceAdapter(
      config,
      sequence(response(session), response(listing)),
    );
    const listed = await adapter.listDocuments();
    if (listed.status !== 'ready') throw new Error('fixture listing was not ready');
    const document = {
      ...listed.documents[0]!,
      source: {
        kind: 'enterprise-catdb' as const,
        id: IDS.source,
        connectionId: 'different-connection',
        deploymentId: IDS.source,
      },
    } satisfies MobileLibraryDocument;

    await expect(adapter.readDocument(document)).rejects.toMatchObject({
      name: MobileSourceReadError.name,
      kind: 'identity-mismatch',
    });
  });
});
