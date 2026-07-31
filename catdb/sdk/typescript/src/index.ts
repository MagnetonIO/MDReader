/**
 * `@catdb/client` — the typed TypeScript SDK for the CatDB platform HTTP API.
 *
 * All types are generated from the committed OpenAPI contract
 * (`catdb/proto/openapi.json` → {@link ./openapi.gen.ts}); nothing here is
 * hand-authored. See {@link createCatdbClient} for the entry point.
 */
export { createCatdbClient } from './client.js';
export type {
  CatdbClient,
  CatdbClientOptions,
  CatdbRequestOptions,
  FetchLike,
} from './client.js';

export {
  CatdbApiError,
  entityExecutionErrorDetails,
  isApiErrorBody,
} from './errors.js';
export type {
  ApiErrorBody,
  CatdbApiErrorInit,
  EntityExecutionErrorDetails,
} from './errors.js';

// Friendly request/response DTO aliases (projected from the generated schemas).
export type {
  ApplyRequest,
  ApplyResponse,
  BuildContextRequest,
  BuildContextResponse,
  CatOperationDto,
  CatalogMappingVersion,
  CatalogPolicyVersion,
  CatalogSchemaVersion,
  CatalogSourceVersion,
  ControlPlaneDiagnostics,
  ControlPlaneSnapshot,
  ContextItemDto,
  EntityFilter,
  EntityQueryRequest,
  EntityQueryResponse,
  EntityRow,
  ExplainRequest,
  ExplainResponse,
  IndexRequest,
  IndexResponse,
  IndexStatusResponse,
  QueryRequest,
  QueryResponse,
  PublicSearchHitDto,
  PublicSearchResponse,
  ReadRequest,
  ReadResponse,
  DocumentDto,
  DocumentsResponse,
  RelatedDocDto,
  RelatedRequest,
  RelatedResponse,
  SearchHitDto,
  SearchRequest,
  SearchResponse,
  SessionResponse,
  SchemaDiffRequest,
  SchemaDiffResponse,
  SourceExecution,
  SourceTestResponse,
  PublishMappingRequest,
  PublishMappingResponse,
  SourceDto,
  SourcesResponse,
  ValidateRequest,
  ValidateResponse,
} from './types.js';

// The full generated surface, for power users who want paths/operations/every
// schema keyed directly.
export type { components, operations, paths } from './openapi.gen.js';
