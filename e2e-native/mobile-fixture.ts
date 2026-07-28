import { createServer, type IncomingMessage, type ServerResponse } from 'node:http';

export const MOBILE_FIXTURE_HOST = '127.0.0.1';
export const MOBILE_FIXTURE_BEARER = 'mdreader-e2e-bearer';
export const MOBILE_FIXTURE_IDS = {
  deployment: '00000000-0000-0000-0000-000000000201',
  workspace: '00000000-0000-0000-0000-000000000202',
  resolver: '00000000-0000-0000-0000-000000000203',
  catalog: '00000000-0000-0000-0000-000000000204',
  schema: '00000000-0000-0000-0000-000000000205',
  source: '00000000-0000-0000-0000-000000000206',
  sourceVersion: '00000000-0000-0000-0000-000000000207',
  excludedSource: '00000000-0000-0000-0000-000000000213',
  excludedSourceVersion: '00000000-0000-0000-0000-000000000214',
  mapping: '00000000-0000-0000-0000-000000000208',
  policy: '00000000-0000-0000-0000-000000000209',
  query: '00000000-0000-0000-0000-000000000210',
  provenanceRule: '00000000-0000-0000-0000-000000000211',
  physicalSchema: '00000000-0000-0000-0000-000000000212',
} as const;

const CONTENT =
  '# Mobile evidence handbook\n\nOffline provenance stays source-bound.!';
const MAX_REQUEST_BYTES = 64 * 1024;

function json(response: ServerResponse, status: number, body: unknown): void {
  const encoded = JSON.stringify(body);
  response.writeHead(status, {
    'content-type': 'application/json',
    'content-length': Buffer.byteLength(encoded),
    'cache-control': 'no-store',
  });
  response.end(encoded);
}

async function requestJson(request: IncomingMessage): Promise<Record<string, unknown>> {
  const chunks: Buffer[] = [];
  let bytes = 0;
  for await (const chunk of request) {
    const buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    bytes += buffer.byteLength;
    if (bytes > MAX_REQUEST_BYTES) throw new Error('fixture request is too large');
    chunks.push(buffer);
  }
  const parsed: unknown = JSON.parse(Buffer.concat(chunks).toString('utf8'));
  if (typeof parsed !== 'object' || parsed === null) throw new Error('fixture request is invalid');
  return parsed as Record<string, unknown>;
}

function provenance(fields: Record<string, unknown>): Record<string, unknown> {
  return Object.fromEntries(
    Object.entries(fields).map(([field, value]) => [
      field,
      {
        field,
        provenance_rule_version_id: MOBILE_FIXTURE_IDS.provenanceRule,
        selected: 0,
        considered: [
          {
            source_version_id: MOBILE_FIXTURE_IDS.sourceVersion,
            record_key: 'mobile-e2e-document',
            mapping_version_id: MOBILE_FIXTURE_IDS.mapping,
            observed_at_ms: 1_700_000_000_000,
            state: { state: 'observed_value', value },
          },
        ],
      },
    ]),
  );
}

function entityResponse(
  fields: Record<string, unknown>,
  retryAfterMs: number,
): Record<string, unknown> {
  return {
    query_id: MOBILE_FIXTURE_IDS.query,
    pins: {
      catalog_resolver_id: MOBILE_FIXTURE_IDS.resolver,
      catalog_revision_id: MOBILE_FIXTURE_IDS.catalog,
      schema_version_id: MOBILE_FIXTURE_IDS.schema,
      physical_schema_version_ids: [MOBILE_FIXTURE_IDS.physicalSchema],
      mapping_version_ids: [MOBILE_FIXTURE_IDS.mapping],
      source_version_ids: [
        MOBILE_FIXTURE_IDS.sourceVersion,
        MOBILE_FIXTURE_IDS.excludedSourceVersion,
      ],
      policy_version_ids: [MOBILE_FIXTURE_IDS.policy],
      provenance_rule_version_id: MOBILE_FIXTURE_IDS.provenanceRule,
    },
    rows: [
      {
        record_key: 'mobile-e2e-document',
        fields,
        provenance: provenance(fields),
      },
    ],
    sources: [
      {
        source_id: MOBILE_FIXTURE_IDS.source,
        source_version_id: MOBILE_FIXTURE_IDS.sourceVersion,
        scanned: 1,
        returned: 1,
        freshness_ms: 0,
        outcome: 'ran',
      },
      {
        source_id: MOBILE_FIXTURE_IDS.excludedSource,
        source_version_id: MOBILE_FIXTURE_IDS.excludedSourceVersion,
        scanned: 0,
        returned: 0,
        freshness_ms: null,
        retry_after_ms: retryAfterMs,
        outcome: 'refused',
      },
    ],
    completeness: {
      state: 'partial',
      excluded_source_ids: [MOBILE_FIXTURE_IDS.excludedSource],
    },
    next_cursor: null,
  };
}

export interface MobileFixtureServer {
  origin: string;
  requests: () => readonly MobileFixtureRequest[];
  close: () => Promise<void>;
}

export interface MobileFixtureRequest {
  method: string;
  path: string;
  authenticated: boolean;
}

/**
 * Start the loopback-only server fixture consumed through CatDB's generated
 * session/entity-query client. The first semantic body query succeeds and
 * later body queries fail, forcing the product through its authorized cache.
 */
export async function startMobileFixtureServer(
  port = 0,
): Promise<MobileFixtureServer> {
  let documentReads = 0;
  let entityQueries = 0;
  const requests: MobileFixtureRequest[] = [];
  const server = createServer((request, response) => {
    void (async () => {
      const authenticated =
        request.headers.authorization === `Bearer ${MOBILE_FIXTURE_BEARER}`;
      const url = new URL(request.url ?? '/', `http://${MOBILE_FIXTURE_HOST}`);
      requests.push({
        method: request.method ?? '',
        path: url.pathname,
        authenticated,
      });
      if (!authenticated) {
        json(response, 403, { error: 'refused', message: 'session refused' });
        return;
      }
      if (request.method === 'GET' && url.pathname === '/v1/session') {
        json(response, 200, {
          deployment_id: MOBILE_FIXTURE_IDS.deployment,
          principal_kind: 'oidc',
          scopes: ['query'],
          workspace_ids: [MOBILE_FIXTURE_IDS.workspace],
        });
        return;
      }
      if (request.method === 'POST' && url.pathname === '/v1/entity/query') {
        entityQueries += 1;
        const query = await requestJson(request);
        const select = query.select;
        if (
          query.workspace_id !== MOBILE_FIXTURE_IDS.workspace ||
          query.entity !== 'document.Document' ||
          !Array.isArray(select)
        ) {
          json(response, 400, { error: 'invalid_query', message: 'invalid fixture query' });
          return;
        }
        const availableFields: Record<string, unknown> = {
          providerId: 'mobile-e2e-document',
          author: 'Fixture Author',
          path: 'fixtures/mobile-e2e.md',
          title: 'Mobile evidence handbook',
          mediaType: 'text/markdown',
          modifiedAt: '2026-07-28T12:00:00Z',
          size: Buffer.byteLength(CONTENT),
          revision: 'fixture-r1',
          checksum: 'sha256:mobile-e2e-document',
          body: CONTENT,
        };
        const fields = Object.fromEntries(
          select
            .filter((field): field is string => typeof field === 'string')
            .filter((field) => Object.hasOwn(availableFields, field))
            .map((field) => [field, availableFields[field]]),
        );
        if (select.includes('body')) {
          const connectedDesktopQuery = Array.isArray(query.source_ids);
          if (!connectedDesktopQuery) documentReads += 1;
          if (!connectedDesktopQuery && documentReads > 1) {
            json(response, 503, {
              error: 'fixture_offline',
              message: 'fixture is offline',
            });
            return;
          }
        }
        json(response, 200, entityResponse(fields, 10_000 + entityQueries));
        return;
      }
      json(response, 404, { error: 'not_found', message: 'fixture route not found' });
    })().catch(() => {
      if (!response.headersSent) {
        json(response, 400, { error: 'invalid_request', message: 'invalid fixture request' });
      } else {
        response.destroy();
      }
    });
  });

  await new Promise<void>((resolve, reject) => {
    server.once('error', reject);
    server.listen(port, MOBILE_FIXTURE_HOST, () => {
      server.off('error', reject);
      resolve();
    });
  });
  const address = server.address();
  if (!address || typeof address === 'string') {
    server.close();
    throw new Error('mobile fixture did not bind a TCP address');
  }
  return {
    origin: `http://${MOBILE_FIXTURE_HOST}:${address.port}`,
    requests: () => requests.map((request) => ({ ...request })),
    close: () =>
      new Promise<void>((resolve, reject) => {
        server.close((error) => (error ? reject(error) : resolve()));
      }),
  };
}
