import {
  CatdbApiError,
  createCatdbClient,
  type EntityQueryRequest,
  type EntityQueryResponse,
  type EntityRow,
  type FetchLike,
  type SessionResponse,
} from '@catdb/client';

import { commands, type MobileEnterpriseError } from '../bindings';
import type { EnterpriseServerConfig } from './source-policy';
import type {
  MobileDocumentIdentity,
  MobileSourceIdentity,
} from './offline-cache';
import { sameMobileDocumentIdentity } from './offline-cache';
import {
  mobileProvenanceOf,
  type MobileDocumentProvenance,
} from './mobile-provenance';

const DOCUMENT_ENTITY = 'document.Document';
const LIST_FIELDS = ['path', 'title', 'size'] as const;
const READ_FIELDS = [...LIST_FIELDS, 'body'] as const;
const MAX_LIBRARY_DOCUMENTS = 1_000;
const MAX_RESPONSE_BYTES = 3 * 1024 * 1024;
const MAX_TEXT_FIELD = 4_096;

export interface MobileLibraryDocument extends MobileDocumentIdentity {
  name: string;
  title: string | null;
  size: number;
}

export type MobileLibraryResult =
  | { status: 'ready'; label: string; documents: MobileLibraryDocument[] }
  | { status: 'refused'; message: string }
  | { status: 'identity-changed'; message: string }
  | { status: 'unavailable'; message: string }
  | { status: 'invalid-response'; message: string };

export type { MobileDocumentProvenance } from './mobile-provenance';

export interface MobileDocumentPayload extends MobileDocumentIdentity {
  content: string;
  revision: string;
  observedAt: number;
  provenance: MobileDocumentProvenance | null;
}

export class MobileSourceReadError extends Error {
  readonly kind:
    | 'refused'
    | 'identity-changed'
    | 'unavailable'
    | 'invalid-response'
    | 'identity-mismatch';

  constructor(
    kind: MobileSourceReadError['kind'],
    message: string,
  ) {
    super(message);
    this.name = 'MobileSourceReadError';
    this.kind = kind;
  }
}

export interface EnterpriseMobileSourceAdapter {
  listDocuments: () => Promise<MobileLibraryResult>;
  readDocument: (document: MobileLibraryDocument) => Promise<MobileDocumentPayload>;
}

type Fetcher = (input: RequestInfo | URL, init?: RequestInit) => Promise<Response>;
const NATIVE_TRANSPORT_ORIGIN = 'http://mdreader-native.invalid';

class ResponseBoundError extends Error {
  constructor() {
    super('CatDB response exceeded the mobile response bound');
    this.name = 'ResponseBoundError';
  }
}

function record(value: unknown): Record<string, unknown> | null {
  return typeof value === 'object' && value !== null
    ? (value as Record<string, unknown>)
    : null;
}

function text(value: unknown, max = MAX_TEXT_FIELD): string | null {
  return typeof value === 'string' && value.length > 0 && value.length <= max ? value : null;
}

function nonNegativeNumber(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0 ? value : null;
}

function basename(path: string): string {
  return path.split(/[\\/]/).filter(Boolean).at(-1) ?? path;
}

async function boundedResponse(response: Response): Promise<Response> {
  const declared = response.headers.get('content-length');
  if (declared !== null) {
    const bytes = Number(declared);
    if (!Number.isSafeInteger(bytes) || bytes < 0 || bytes > MAX_RESPONSE_BYTES) {
      throw new ResponseBoundError();
    }
  }
  if (!response.body) return response;

  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let total = 0;
  for (;;) {
    const next = await reader.read();
    if (next.done) break;
    total += next.value.byteLength;
    if (total > MAX_RESPONSE_BYTES) {
      await reader.cancel();
      throw new ResponseBoundError();
    }
    chunks.push(next.value);
  }
  const body = new Uint8Array(total);
  let offset = 0;
  for (const chunk of chunks) {
    body.set(chunk, offset);
    offset += chunk.byteLength;
  }
  const bodyInit =
    response.status === 204 || response.status === 205 || response.status === 304
      ? null
      : body;
  return new Response(bodyInit, {
    status: response.status,
    statusText: response.statusText,
    headers: response.headers,
  });
}

function boundedFetch(fetcher: Fetcher): FetchLike {
  return async (input, init) => {
    return boundedResponse(await fetcher(input, init));
  };
}

class NativeEnterpriseError extends Error {
  constructor(readonly detail: MobileEnterpriseError) {
    super(detail.kind);
    this.name = 'NativeEnterpriseError';
  }
}

function nativeFetch(config: EnterpriseServerConfig): FetchLike {
  return async (input, init) => {
    const url = new URL(typeof input === 'string' ? input : input.toString());
    const method = init?.method ?? 'GET';
    const operation =
      url.pathname === '/v1/session' && method === 'GET'
        ? 'session'
        : url.pathname === '/v1/entity/query' && method === 'POST'
          ? 'entity-query'
          : null;
    if (!operation) throw new NativeEnterpriseError({ kind: 'invalid-request' });
    const bodyJson =
      operation === 'entity-query' && typeof init?.body === 'string' ? init.body : null;
    const result = await commands.mobileEnterpriseRequest(
      config.connectionId,
      operation,
      bodyJson,
    );
    if (result.status === 'error') throw new NativeEnterpriseError(result.error);
    return new Response(result.data.body_json, {
      status: result.data.status,
      headers: { 'content-type': 'application/json' },
    });
  };
}

function validSession(value: SessionResponse): SessionResponse | null {
  const candidate = record(value);
  if (
    !candidate ||
    !text(candidate.deployment_id, 1_024) ||
    !text(candidate.principal_kind, 128) ||
    !Array.isArray(candidate.scopes) ||
    !Array.isArray(candidate.workspace_ids) ||
    candidate.scopes.some((scope) => !text(scope, 128)) ||
    candidate.workspace_ids.some((workspace) => !text(workspace, 1_024))
  ) {
    return null;
  }
  return value;
}

function validQueryResponse(value: EntityQueryResponse): EntityQueryResponse | null {
  const candidate = record(value);
  const pins = record(candidate?.pins);
  if (
    !candidate ||
    !text(candidate.query_id, 1_024) ||
    !pins ||
    !text(pins.catalog_resolver_id, 1_024) ||
    !text(pins.catalog_revision_id, 1_024) ||
    !text(pins.schema_version_id, 1_024) ||
    !text(pins.provenance_rule_version_id, 1_024) ||
    !Array.isArray(pins.mapping_version_ids) ||
    !Array.isArray(pins.physical_schema_version_ids) ||
    !Array.isArray(pins.policy_version_ids) ||
    !Array.isArray(pins.source_version_ids) ||
    !Array.isArray(candidate.rows) ||
    !Array.isArray(candidate.sources) ||
    !record(candidate.completeness)
  ) {
    return null;
  }
  for (const versions of [
    pins.mapping_version_ids,
    pins.physical_schema_version_ids,
    pins.policy_version_ids,
    pins.source_version_ids,
  ]) {
    if (versions.length > 1_000 || versions.some((version) => !text(version, 1_024))) {
      return null;
    }
  }
  return value;
}

type SelectedObservation = {
  sourceVersionId: string;
  recordKey: string;
  mappingVersionId: string;
  observedAt: number | null;
};

function selectedObservation(
  row: EntityRow,
  field: string,
  provenanceRuleVersionId: string,
): SelectedObservation | null {
  const provenance = record(row.provenance[field]);
  const selected = provenance?.selected;
  const considered = provenance?.considered;
  if (
    provenance?.field !== field ||
    provenance.provenance_rule_version_id !== provenanceRuleVersionId ||
    !Number.isInteger(selected) ||
    typeof selected !== 'number' ||
    !Array.isArray(considered) ||
    considered.length > 64 ||
    selected < 0 ||
    selected >= considered.length
  ) {
    return null;
  }
  const observation = record(considered[selected]);
  const sourceVersionId = text(observation?.source_version_id, 1_024);
  const recordKey = text(observation?.record_key, 1_024);
  const mappingVersionId = text(observation?.mapping_version_id, 1_024);
  const observedAt =
    observation?.observed_at_ms === null || observation?.observed_at_ms === undefined
      ? null
      : nonNegativeNumber(observation.observed_at_ms);
  const state = record(observation?.state);
  const returned = row.fields[field];
  const stateMatches =
    (returned === null && state?.state === 'observed_null') ||
    (state?.state === 'observed_value' && Object.is(state.value, returned));
  if (
    !sourceVersionId ||
    !recordKey ||
    !mappingVersionId ||
    (observation?.observed_at_ms !== null &&
      observation?.observed_at_ms !== undefined &&
      observedAt === null) ||
    !stateMatches
  ) {
    return null;
  }
  return { sourceVersionId, recordKey, mappingVersionId, observedAt };
}

function sourceIdFor(
  response: EntityQueryResponse,
  sourceVersionId: string,
): string | null {
  const matches = response.sources.filter(
    (source) => source.source_version_id === sourceVersionId,
  );
  return matches.length === 1 ? text(matches[0]?.source_id, 1_024) : null;
}

function documentOf(
  response: EntityQueryResponse,
  workspaceId: string,
  source: Extract<MobileSourceIdentity, { kind: 'enterprise-catdb' }>,
  row: EntityRow,
): MobileLibraryDocument | null {
  const rowRecord = record(row);
  const fields = record(rowRecord?.fields);
  const provenance = record(rowRecord?.provenance);
  const recordKey = text(rowRecord?.record_key, 1_024);
  if (!fields || !provenance || !recordKey) return null;
  const path = text(fields.path);
  const title = fields.title === null ? null : text(fields.title, 1_024);
  const size = nonNegativeNumber(fields.size);
  const provenanceRuleVersionId = response.pins.provenance_rule_version_id;
  const pathObservation = selectedObservation(row, 'path', provenanceRuleVersionId);
  const titleObservation = selectedObservation(row, 'title', provenanceRuleVersionId);
  const sizeObservation = selectedObservation(row, 'size', provenanceRuleVersionId);
  if (
    !path ||
    (fields.title !== null && !title) ||
    size === null ||
    !pathObservation ||
    !titleObservation ||
    !sizeObservation ||
    [pathObservation, titleObservation, sizeObservation].some(
      (observation) =>
        observation.recordKey !== recordKey ||
        !response.pins.source_version_ids.includes(observation.sourceVersionId) ||
        !response.pins.mapping_version_ids.includes(observation.mappingVersionId) ||
        sourceIdFor(response, observation.sourceVersionId) === null,
    )
  ) {
    return null;
  }
  return {
    source,
    sourceRoot: { id: workspaceId, version: response.pins.catalog_revision_id },
    document: { id: recordKey, path },
    name: basename(path),
    title,
    size,
  };
}

function listRequest(workspaceId: string): EntityQueryRequest {
  return {
    workspace_id: workspaceId,
    entity: DOCUMENT_ENTITY,
    select: [...LIST_FIELDS],
    filters: [],
    order_by: [{ field: 'path', direction: 'ascending' }],
    page_size: MAX_LIBRARY_DOCUMENTS,
    limit: MAX_LIBRARY_DOCUMENTS,
    require_complete: true,
  };
}

function readRequest(document: MobileLibraryDocument): EntityQueryRequest {
  return {
    workspace_id: document.sourceRoot.id,
    catalog_revision_id: document.sourceRoot.version,
    entity: DOCUMENT_ENTITY,
    select: [...READ_FIELDS],
    filters: [{ field: 'path', operator: 'eq', value: document.document.path }],
    order_by: [],
    page_size: 2,
    limit: 2,
    require_complete: true,
  };
}

function libraryFailure(cause: unknown): MobileLibraryResult {
  if (cause instanceof CatdbApiError && (cause.status === 401 || cause.status === 403)) {
    return { status: 'refused', message: 'The Enterprise session was refused.' };
  }
  if (
    cause instanceof NativeEnterpriseError &&
    cause.detail.kind === 'connection-identity-changed'
  ) {
    return {
      status: 'identity-changed',
      message:
        'The Enterprise connection identity changed. Reconnect and verify the server and workspace before trying again.',
    };
  }
  if (cause instanceof ResponseBoundError) {
    return {
      status: 'invalid-response',
      message: 'The Enterprise server response exceeded the mobile safety bound.',
    };
  }
  if (cause instanceof NativeEnterpriseError && cause.detail.kind === 'response-too-large') {
    return {
      status: 'invalid-response',
      message: 'The Enterprise server response exceeded the mobile safety bound.',
    };
  }
  return { status: 'unavailable', message: 'The Enterprise server is unavailable.' };
}

function readFailure(cause: unknown): MobileSourceReadError {
  if (cause instanceof MobileSourceReadError) return cause;
  if (cause instanceof CatdbApiError && (cause.status === 401 || cause.status === 403)) {
    return new MobileSourceReadError('refused', 'The Enterprise session was refused.');
  }
  if (
    cause instanceof NativeEnterpriseError &&
    cause.detail.kind === 'connection-identity-changed'
  ) {
    return new MobileSourceReadError(
      'identity-changed',
      'The Enterprise connection identity changed. Reconnect and verify the server and workspace before reading this document again.',
    );
  }
  if (cause instanceof ResponseBoundError) {
    return new MobileSourceReadError(
      'invalid-response',
      'The Enterprise server response exceeded the mobile safety bound.',
    );
  }
  return new MobileSourceReadError(
    'unavailable',
    'The Enterprise document is unavailable.',
  );
}

async function sha256(content: string): Promise<string> {
  const digest = await crypto.subtle.digest('SHA-256', new TextEncoder().encode(content));
  return `sha256:${Array.from(new Uint8Array(digest), (byte) =>
    byte.toString(16).padStart(2, '0'),
  ).join('')}`;
}

/**
 * Bind a validated server-session reference to CatDB's generated, authenticated
 * `/v1/session` and `/v1/entity/query` contracts. Legacy remote document reads
 * remain fail-closed; the semantic `document.Document` schema is the only
 * server path consumed here.
 */
export function createEnterpriseMobileSourceAdapter(
  config: EnterpriseServerConfig,
  fetcher?: Fetcher,
): EnterpriseMobileSourceAdapter {
  const client = createCatdbClient({
    baseUrl: NATIVE_TRANSPORT_ORIGIN,
    fetch: fetcher ? boundedFetch(fetcher) : nativeFetch(config),
  });
  let admitted:
    | {
        workspaceId: string;
        source: Extract<MobileSourceIdentity, { kind: 'enterprise-catdb' }>;
      }
    | null = null;

  return {
    async listDocuments() {
      try {
        admitted = null;
        const session = validSession(await client.session());
        if (!session) {
          return {
            status: 'invalid-response',
            message: 'The Enterprise server returned an invalid session response.',
          };
        }
        if (!session.scopes.includes('query')) {
          return {
            status: 'refused',
            message: 'The Enterprise session does not allow document queries.',
          };
        }
        if (session.workspace_ids.length !== 1) {
          return {
            status: 'invalid-response',
            message: 'Choose an Enterprise session restricted to one workspace.',
          };
        }
        const workspaceId = session.workspace_ids[0]!;
        const source = {
          kind: 'enterprise-catdb' as const,
          id: session.deployment_id,
          connectionId: config.connectionId,
          deploymentId: session.deployment_id,
        };
        const response = validQueryResponse(await client.entityQuery(listRequest(workspaceId)));
        if (
          !response ||
          response.rows.length > MAX_LIBRARY_DOCUMENTS ||
          response.next_cursor != null ||
          response.completeness.state !== 'complete' ||
          !mobileProvenanceOf({
            pins: response.pins,
            fields: {},
            sources: response.sources,
          })
        ) {
          return {
            status: 'invalid-response',
            message: 'The Enterprise server returned an invalid semantic response.',
          };
        }
        const documents: MobileLibraryDocument[] = [];
        for (const row of response.rows) {
          const provenance = mobileProvenanceOf(
            {
              pins: response.pins,
              fields: row.provenance,
              sources: response.sources,
            },
            row.record_key,
          );
          const document = documentOf(response, workspaceId, source, row);
          if (!provenance || !document) {
            return {
              status: 'invalid-response',
              message: 'The Enterprise server returned an invalid semantic response.',
            };
          }
          documents.push(document);
        }
        admitted = { workspaceId, source };
        return {
          status: 'ready',
          label: 'Enterprise Document library',
          documents,
        };
      } catch (cause) {
        return libraryFailure(cause);
      }
    },

    async readDocument(document) {
      try {
        if (
          !admitted ||
          admitted.workspaceId !== document.sourceRoot.id ||
          !sameMobileDocumentIdentity(
            document,
            {
              ...document,
              source: admitted.source,
              sourceRoot: {
                ...document.sourceRoot,
                id: admitted.workspaceId,
              },
            },
          )
        ) {
          throw new MobileSourceReadError(
            'identity-mismatch',
            'The admitted Enterprise service identity changed during the read.',
          );
        }
        const response = validQueryResponse(await client.entityQuery(readRequest(document)));
        if (
          !response ||
          response.completeness.state !== 'complete' ||
          response.next_cursor != null ||
          response.rows.length !== 1 ||
          response.pins.catalog_revision_id !== document.sourceRoot.version
        ) {
          throw new MobileSourceReadError(
            'invalid-response',
            'The Enterprise server returned an invalid semantic response.',
          );
        }
        const row = response.rows[0]!;
        const actual = documentOf(
          response,
          document.sourceRoot.id,
          admitted.source,
          row,
        );
        const body =
          typeof row.fields.body === 'string' && row.fields.body.length <= 2 * 1024 * 1024
            ? row.fields.body
            : null;
        const bodyObservation = selectedObservation(
          row,
          'body',
          response.pins.provenance_rule_version_id,
        );
        if (
          !actual ||
          body === null ||
          !bodyObservation ||
          bodyObservation.observedAt === null ||
          !response.pins.source_version_ids.includes(bodyObservation.sourceVersionId) ||
          !response.pins.mapping_version_ids.includes(bodyObservation.mappingVersionId)
        ) {
          throw new MobileSourceReadError(
            'invalid-response',
            'The Enterprise server returned an invalid semantic response.',
          );
        }
        if (!sameMobileDocumentIdentity(document, actual)) {
          throw new MobileSourceReadError(
            'identity-mismatch',
            'The Enterprise document identity changed during the read.',
          );
        }
        const bodySource = sourceIdFor(response, bodyObservation.sourceVersionId);
        if (
          bodySource === null ||
          bodyObservation.recordKey !== document.document.id
        ) {
          throw new MobileSourceReadError(
            'identity-mismatch',
            'The Enterprise document provider identity is invalid.',
          );
        }
        const provenance = mobileProvenanceOf(
          {
            pins: response.pins,
            fields: row.provenance,
            sources: response.sources,
          },
          row.record_key,
        );
        if (!provenance || response.pins.policy_version_ids.length === 0) {
          throw new MobileSourceReadError(
            'invalid-response',
            'The Enterprise server returned incomplete provenance.',
          );
        }
        return {
          ...document,
          content: body,
          revision: await sha256(body),
          observedAt: bodyObservation.observedAt,
          provenance,
        };
      } catch (cause) {
        throw readFailure(cause);
      }
    },
  };
}
