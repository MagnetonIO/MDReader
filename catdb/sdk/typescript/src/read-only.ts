/**
 * Browser-safe CatDB client for the physically read-only public surface.
 *
 * This is a separate package entry rather than a narrowed value imported from
 * the full SDK. Its emitted module therefore contains no mutation, indexing,
 * validation, planning, or context-building route strings for a public bundle
 * to retain accidentally.
 */

import { CatdbApiError } from './errors.js';
import type {
  DocumentsResponse,
  PublicSearchResponse,
  ReadRequest,
  ReadResponse,
  RelatedRequest,
  RelatedResponse,
  SearchRequest,
  SourcesResponse,
} from './types.js';

/** A `fetch`-compatible function. */
export type ReadOnlyFetchLike = typeof fetch;

/** Options for the public read-only CatDB client. */
export interface ReadOnlyCatdbClientOptions {
  /** CatDB origin or same-origin proxy prefix. */
  baseUrl: string;
  /** Custom fetch implementation. */
  fetch?: ReadOnlyFetchLike;
  /** Headers merged into every request. */
  headers?: ConstructorParameters<typeof Headers>[0];
}

/** The five operations published by the public CatDB demo. */
export interface ReadOnlyCatdbClient {
  /** Hybrid search over the indexed corpus. */
  search(body: SearchRequest): Promise<PublicSearchResponse>;
  /** Read one indexed document and its sections. */
  read(body: ReadRequest): Promise<ReadResponse>;
  /** Find related documents for an indexed anchor. */
  related(body: RelatedRequest): Promise<RelatedResponse>;
  /** Enumerate public-safe source identities. */
  sources(): Promise<SourcesResponse>;
  /** Enumerate indexed documents. */
  documents(): Promise<DocumentsResponse>;
}

async function parseOkBody(response: Response): Promise<unknown> {
  const raw = response.status === 204 || response.status === 205 ? '' : await response.text();
  if (raw.trim().length === 0) {
    throw new CatdbApiError(
      `CatDB returned an empty body on a successful response (HTTP ${response.status})`,
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

/**
 * Create a client whose runtime and type surfaces contain only public reads.
 */
export function createReadOnlyCatdbClient(
  options: ReadOnlyCatdbClientOptions,
): ReadOnlyCatdbClient {
  if (!options.baseUrl) {
    throw new TypeError('createReadOnlyCatdbClient: `baseUrl` is required');
  }
  const baseUrl = options.baseUrl.replace(/\/+$/, '');
  const fallback =
    typeof globalThis.fetch === 'function' ? globalThis.fetch.bind(globalThis) : undefined;
  const fetchImpl = options.fetch ?? (fallback as ReadOnlyFetchLike);
  if (typeof fetchImpl !== 'function') {
    throw new TypeError(
      'createReadOnlyCatdbClient: no `fetch` available — pass `fetch` in the options',
    );
  }
  const baseHeaders = new Headers(options.headers);

  const request = async <ResponseBody>(
    method: 'GET' | 'POST',
    path: string,
    body?: unknown,
  ): Promise<ResponseBody> => {
    const headers = new Headers(baseHeaders);
    headers.set('accept', 'application/json');
    const init: RequestInit = { method, headers };
    if (body !== undefined) {
      headers.set('content-type', 'application/json');
      init.body = JSON.stringify(body);
    }
    const response = await fetchImpl(`${baseUrl}${path}`, init);
    if (!response.ok) throw await CatdbApiError.fromResponse(response);
    return (await parseOkBody(response)) as ResponseBody;
  };

  return Object.freeze({
    search: (body: SearchRequest) =>
      request<PublicSearchResponse>('POST', '/v1/search', body),
    read: (body: ReadRequest) =>
      request<ReadResponse>('POST', '/v1/read', body),
    related: (body: RelatedRequest) =>
      request<RelatedResponse>('POST', '/v1/related', body),
    sources: () => request<SourcesResponse>('GET', '/v1/sources'),
    documents: () => request<DocumentsResponse>('GET', '/v1/documents'),
  });
}
