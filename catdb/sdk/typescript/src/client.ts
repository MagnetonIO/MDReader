import { CatdbApiError } from './errors.js';
import type { operations } from './openapi.gen.js';
import type {
  ApplyRequest,
  ApplyResponse,
  BuildContextRequest,
  BuildContextResponse,
  ControlPlaneDiagnostics,
  ControlPlaneSnapshot,
  EntityQueryRequest,
  EntityQueryResponse,
  ExplainRequest,
  ExplainResponse,
  IndexRequest,
  IndexResponse,
  IndexStatusResponse,
  QueryRequest,
  QueryResponse,
  ReadRequest,
  ReadResponse,
  DocumentsResponse,
  RelatedRequest,
  RelatedResponse,
  SearchRequest,
  SearchResponse,
  SessionResponse,
  SchemaDiffRequest,
  SchemaDiffResponse,
  SourceTestResponse,
  PublishMappingRequest,
  PublishMappingResponse,
  SourcesResponse,
  ValidateRequest,
  ValidateResponse,
} from './types.js';

/**
 * A `fetch`-compatible function. Defaults to the runtime's global `fetch`;
 * inject one (e.g. a Node polyfill, an instrumented wrapper, or a test double)
 * via {@link CatdbClientOptions.fetch}.
 */
export type FetchLike = typeof fetch;

/** Options for {@link createCatdbClient}. */
export interface CatdbClientOptions {
  /**
   * Base URL of the catdb-server (e.g. `http://127.0.0.1:8080`). A trailing
   * slash is tolerated; endpoint paths (`/v1/...`) are appended to it.
   */
  baseUrl: string;
  /** Custom `fetch` implementation (defaults to the global `fetch`). */
  fetch?: FetchLike;
  /** Headers merged into every request (e.g. `Authorization`). */
  headers?: ConstructorParameters<typeof Headers>[0];
}

/** Per-request controls that do not alter the typed CatDB payload. */
export interface CatdbRequestOptions {
  /** Abort waiting for the HTTP operation. */
  signal?: AbortSignal;
}

// --- Operation → JSON body / 200-response projections -----------------------
// These pull the request-body and success-response shapes straight out of the
// generated `operations` type, so the client stays in lockstep with the spec.

type JsonBody<O> = O extends { requestBody: { content: { 'application/json': infer B } } }
  ? B
  : never;

type JsonOk<O> = O extends {
  responses: { 200: { content: { 'application/json': infer R } } };
}
  ? R
  : never;

/**
 * A typed client over the catdb-server HTTP surface. One method per `/v1`
 * endpoint. Each method resolves to the typed 200 body, and throws a
 * {@link CatdbApiError} for any non-2xx response, or for a 2xx that violates
 * the contract (no body, or a body that is not valid JSON).
 *
 * Return types are deliberately NON-optional: every unary endpoint declares a 200
 * JSON body in the OpenAPI contract, so a missing body is a protocol
 * violation to be surfaced loudly — not an outcome every call site has to
 * defend against with `?.`/`?? []`, which would silently turn a broken
 * response into "no results".
 */
export interface CatdbClient {
  /** `POST /v1/search` — hybrid search over the corpus. */
  search(body: SearchRequest, options?: CatdbRequestOptions): Promise<SearchResponse>;
  /** `POST /v1/query` — structured query execution. */
  query(body: QueryRequest, options?: CatdbRequestOptions): Promise<QueryResponse>;
  /** `POST /v1/entity/query` — version-pinned semantic entity execution. */
  entityQuery(body: EntityQueryRequest, options?: CatdbRequestOptions): Promise<EntityQueryResponse>;
  /** `POST /v1/entity/explain` — the same answer with its execution evidence. */
  entityExplain(
    body: EntityQueryRequest,
    options?: CatdbRequestOptions,
  ): Promise<EntityQueryResponse>;
  /** Read token-free session identity and exact capabilities. */
  session(options?: CatdbRequestOptions): Promise<SessionResponse>;
  /** Read one immutable catalog snapshot for the Console. */
  controlCatalog(
    workspaceId: string,
    options?: CatdbRequestOptions,
  ): Promise<ControlPlaneSnapshot>;
  /** Run one bounded connector-specific source probe. */
  controlTestSource(
    workspaceId: string,
    sourceId: string,
    options?: CatdbRequestOptions,
  ): Promise<SourceTestResponse>;
  /** Compare two immutable semantic schema versions. */
  controlSchemaDiff(
    body: SchemaDiffRequest,
    options?: CatdbRequestOptions,
  ): Promise<SchemaDiffResponse>;
  /** Publish one validated, non-active mapping candidate. */
  controlPublishMapping(
    body: PublishMappingRequest,
    options?: CatdbRequestOptions,
  ): Promise<PublishMappingResponse>;
  /** Read bounded, redacted runtime diagnostics. */
  controlDiagnostics(
    workspaceId: string,
    options?: CatdbRequestOptions,
  ): Promise<ControlPlaneDiagnostics>;
  /** `POST /v1/read` — read a document and its sections. */
  read(body: ReadRequest, options?: CatdbRequestOptions): Promise<ReadResponse>;
  /** `POST /v1/related` — related documents for an anchor. */
  related(body: RelatedRequest, options?: CatdbRequestOptions): Promise<RelatedResponse>;
  /** `GET /v1/sources` — enumerate public-safe configured sources. */
  sources(options?: CatdbRequestOptions): Promise<SourcesResponse>;
  /** `GET /v1/documents` — enumerate indexed documents. */
  documents(options?: CatdbRequestOptions): Promise<DocumentsResponse>;
  /** `POST /v1/explain` — compile a query into its search plan (no execution). */
  explain(body: ExplainRequest, options?: CatdbRequestOptions): Promise<ExplainResponse>;
  /** `POST /v1/validate` — validate against the semantic contract. */
  validate(body: ValidateRequest, options?: CatdbRequestOptions): Promise<ValidateResponse>;
  /** `POST /v1/index` — (re)build the index over a root. */
  index(body: IndexRequest, options?: CatdbRequestOptions): Promise<IndexResponse>;
  /** `GET /v1/index_status` — current ingest-job progress. */
  indexStatus(options?: CatdbRequestOptions): Promise<IndexStatusResponse>;
  /** `POST /v1/apply` — apply an atomic, validated batch of operations. */
  apply(body: ApplyRequest, options?: CatdbRequestOptions): Promise<ApplyResponse>;
  /** `POST /v1/build_context` — assemble token-budgeted context for a task. */
  buildContext(
    body: BuildContextRequest,
    options?: CatdbRequestOptions,
  ): Promise<BuildContextResponse>;
}

/**
 * Parse the body of an already-known-successful response.
 *
 * Every endpoint declares a 200 JSON body, so both failure modes here are
 * protocol violations (a misbehaving proxy or gateway, most likely) and both
 * raise a {@link CatdbApiError} carrying the status and the raw text — the
 * same treatment the non-2xx path gives, and never a bare `SyntaxError`:
 *
 * - **no body at all** (`204`/`205`, or an empty/whitespace body), and
 * - **a body that is present but not valid JSON**.
 */
async function parseOkBody(response: Response): Promise<unknown> {
  // 204 No Content / 205 Reset Content are defined to carry no body — but the
  // API declares one for every endpoint, so receiving one is still a violation.
  const raw = response.status === 204 || response.status === 205 ? '' : await response.text();
  if (raw.trim().length === 0) {
    throw new CatdbApiError(
      `CatDB returned an empty body on a successful response (HTTP ${response.status}) ` +
        'where the API declares a JSON body — the response was truncated or rewritten in transit',
      { status: response.status, raw },
    );
  }
  try {
    return JSON.parse(raw) as unknown;
  } catch {
    throw new CatdbApiError(
      `CatDB returned a non-JSON body on a successful response (HTTP ${response.status})`,
      { status: response.status, raw },
    );
  }
}

class CatdbHttpClient implements CatdbClient {
  readonly #baseUrl: string;
  readonly #fetch: FetchLike;
  readonly #headers: Headers;

  constructor(options: CatdbClientOptions) {
    if (!options.baseUrl) {
      throw new TypeError('createCatdbClient: `baseUrl` is required');
    }
    // Normalise: strip a single trailing slash so `${base}${path}` is clean.
    this.#baseUrl = options.baseUrl.replace(/\/+$/, '');
    // Bind the default: browsers implement `fetch` as a method of the global and
    // reject a detached call with `TypeError: Illegal invocation`. Since it is
    // later invoked as `this.#fetch(...)`, an unbound capture would re-bind the
    // receiver to this client instance and break every browser consumer (Node's
    // fetch is receiver-agnostic, so unit tests alone would not catch it).
    const fallback =
      typeof globalThis.fetch === 'function'
        ? globalThis.fetch.bind(globalThis)
        : undefined;
    this.#fetch = options.fetch ?? (fallback as FetchLike);
    if (typeof this.#fetch !== 'function') {
      throw new TypeError(
        'createCatdbClient: no `fetch` available — pass `fetch` in the options',
      );
    }
    this.#headers = new Headers(options.headers);
  }

  async #request<O>(
    method: 'GET' | 'POST',
    path: string,
    body?: JsonBody<O>,
    options?: CatdbRequestOptions,
  ): Promise<JsonOk<O>> {
    const headers = new Headers(this.#headers);
    headers.set('accept', 'application/json');
    const init: RequestInit = { method, headers, signal: options?.signal };
    if (body !== undefined) {
      headers.set('content-type', 'application/json');
      init.body = JSON.stringify(body);
    }

    const response = await this.#fetch(`${this.#baseUrl}${path}`, init);
    if (!response.ok) {
      throw await CatdbApiError.fromResponse(response);
    }
    // parseOkBody throws unless a well-formed JSON body actually arrived, so
    // this cast never conjures a value that isn't there.
    return (await parseOkBody(response)) as JsonOk<O>;
  }

  search(body: SearchRequest, options?: CatdbRequestOptions): Promise<SearchResponse> {
    return this.#request<operations['search_handler']>('POST', '/v1/search', body, options);
  }
  query(body: QueryRequest, options?: CatdbRequestOptions): Promise<QueryResponse> {
    return this.#request<operations['query_handler']>('POST', '/v1/query', body, options);
  }
  entityQuery(
    body: EntityQueryRequest,
    options?: CatdbRequestOptions,
  ): Promise<EntityQueryResponse> {
    return this.#request<operations['entity_query_handler']>(
      'POST',
      '/v1/entity/query',
      body,
      options,
    );
  }
  entityExplain(
    body: EntityQueryRequest,
    options?: CatdbRequestOptions,
  ): Promise<EntityQueryResponse> {
    return this.#request<operations['entity_explain_handler']>(
      'POST',
      '/v1/entity/explain',
      body,
      options,
    );
  }
  session(options?: CatdbRequestOptions): Promise<SessionResponse> {
    return this.#request<operations['session_handler']>(
      'GET',
      '/v1/session',
      undefined,
      options,
    );
  }
  sources(options?: CatdbRequestOptions): Promise<SourcesResponse> {
    return this.#request<operations['sources_handler']>('GET', '/v1/sources', undefined, options);
  }
  documents(options?: CatdbRequestOptions): Promise<DocumentsResponse> {
    return this.#request<operations['documents_handler']>(
      'GET',
      '/v1/documents',
      undefined,
      options,
    );
  }
  controlCatalog(
    workspaceId: string,
    options?: CatdbRequestOptions,
  ): Promise<ControlPlaneSnapshot> {
    return this.#request<operations['catalog_snapshot_handler']>(
      'GET',
      `/v1/control/workspaces/${encodeURIComponent(workspaceId)}/catalog`,
      undefined,
      options,
    );
  }
  controlTestSource(
    workspaceId: string,
    sourceId: string,
    options?: CatdbRequestOptions,
  ): Promise<SourceTestResponse> {
    return this.#request<operations['source_test_handler']>(
      'POST',
      `/v1/control/workspaces/${encodeURIComponent(workspaceId)}/sources/${encodeURIComponent(sourceId)}/test`,
      undefined,
      options,
    );
  }
  controlSchemaDiff(
    body: SchemaDiffRequest,
    options?: CatdbRequestOptions,
  ): Promise<SchemaDiffResponse> {
    return this.#request<operations['schema_diff_handler']>(
      'POST',
      '/v1/control/schemas/diff',
      body,
      options,
    );
  }
  controlPublishMapping(
    body: PublishMappingRequest,
    options?: CatdbRequestOptions,
  ): Promise<PublishMappingResponse> {
    return this.#request<operations['publish_mapping_handler']>(
      'POST',
      '/v1/control/mappings',
      body,
      options,
    );
  }
  controlDiagnostics(
    workspaceId: string,
    options?: CatdbRequestOptions,
  ): Promise<ControlPlaneDiagnostics> {
    return this.#request<operations['diagnostics_handler']>(
      'GET',
      `/v1/control/workspaces/${encodeURIComponent(workspaceId)}/diagnostics`,
      undefined,
      options,
    );
  }
  read(body: ReadRequest, options?: CatdbRequestOptions): Promise<ReadResponse> {
    return this.#request<operations['read_handler']>('POST', '/v1/read', body, options);
  }
  related(body: RelatedRequest, options?: CatdbRequestOptions): Promise<RelatedResponse> {
    return this.#request<operations['related_handler']>('POST', '/v1/related', body, options);
  }
  explain(body: ExplainRequest, options?: CatdbRequestOptions): Promise<ExplainResponse> {
    return this.#request<operations['explain_handler']>('POST', '/v1/explain', body, options);
  }
  validate(body: ValidateRequest, options?: CatdbRequestOptions): Promise<ValidateResponse> {
    return this.#request<operations['validate_handler']>('POST', '/v1/validate', body, options);
  }
  index(body: IndexRequest, options?: CatdbRequestOptions): Promise<IndexResponse> {
    return this.#request<operations['index_handler']>('POST', '/v1/index', body, options);
  }
  indexStatus(options?: CatdbRequestOptions): Promise<IndexStatusResponse> {
    return this.#request<operations['index_status_handler']>(
      'GET',
      '/v1/index_status',
      undefined,
      options,
    );
  }
  apply(body: ApplyRequest, options?: CatdbRequestOptions): Promise<ApplyResponse> {
    return this.#request<operations['apply_handler']>('POST', '/v1/apply', body, options);
  }
  buildContext(
    body: BuildContextRequest,
    options?: CatdbRequestOptions,
  ): Promise<BuildContextResponse> {
    return this.#request<operations['build_context_handler']>(
      'POST',
      '/v1/build_context',
      body,
      options,
    );
  }
}

// TODO(stream): the server also exposes two Server-Sent-Events variants —
// `POST /v1/search/stream` (one `hit` event per SearchHitDto, then `done`) and
// `POST /v1/index/stream` (index-build `progress` ticks, then a terminal
// `done`/`error`). They are intentionally NOT part of v1 of this client: they
// are absent from the committed `catdb/proto/openapi.json` (which documents
// only unary `/v1` paths), so there is no generated type to key an
// honest typed async-iterator to. Rather than hand-fabricate the event shapes,
// they are omitted here. When those routes are added to the OpenAPI spec, add
// a typed `AsyncIterable` reusing the generated `SearchHitDto` / status types.

/**
 * Create a typed client for the CatDB platform HTTP API. Each method issues a
 * single request to the corresponding `/v1` endpoint and resolves to the typed
 * response body, or rejects with a {@link CatdbApiError} on any non-2xx status.
 */
export function createCatdbClient(options: CatdbClientOptions): CatdbClient {
  return new CatdbHttpClient(options);
}
