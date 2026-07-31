/**
 * Friendly named aliases for the request/response DTOs, projected out of the
 * generated OpenAPI `components`. These are *aliases*, not re-declarations —
 * the single source of truth remains {@link ./openapi.gen.ts}, which is
 * generated from `catdb/proto/openapi.json` and diff-checked in CI. For the
 * full typed surface (paths / operations / every schema), import
 * `components`, `paths`, and `operations` from the package root.
 */
import type { components } from './openapi.gen.js';

type Schemas = components['schemas'];

// --- /v1/search ---
export type SearchRequest = Schemas['SearchRequest'];
export type SearchResponse = Schemas['SearchResponse'];
export type SearchHitDto = Schemas['SearchHitDto'];
export type PublicSearchResponse = Schemas['PublicSearchResponse'];
export type PublicSearchHitDto = Schemas['PublicSearchHitDto'];

// --- /v1/query ---
export type QueryRequest = Schemas['QueryRequest'];
export type QueryResponse = Schemas['QueryResponse'];

// --- /v1/entity/query + /v1/entity/explain ---
export type EntityQueryRequest = Schemas['EntityQueryRequest'];
export type EntityQueryResponse = Schemas['EntityQueryResponse'];
export type EntityFilter = Schemas['EntityFilter'];
export type EntityRow = Schemas['EntityRow'];
export type SourceExecution = Schemas['SourceExecution'];

// --- /v1/control ---
export type ControlPlaneSnapshot = Schemas['ControlPlaneSnapshot'];
export type CatalogSchemaVersion = Schemas['CatalogSchemaVersion'];
export type CatalogMappingVersion = Schemas['CatalogMappingVersion'];
export type CatalogSourceVersion = Schemas['CatalogSourceVersion'];
export type CatalogPolicyVersion = Schemas['CatalogPolicyVersion'];
export type SourceTestResponse = Schemas['SourceTestResponse'];
export type SchemaDiffRequest = Schemas['SchemaDiffRequest'];
export type SchemaDiffResponse = Schemas['SchemaDiffResponse'];
export type PublishMappingRequest = Schemas['PublishMappingRequest'];
export type PublishMappingResponse = Schemas['PublishMappingResponse'];
export type ControlPlaneDiagnostics = Schemas['ControlPlaneDiagnostics'];
export type SessionResponse = Schemas['SessionResponse'];

// --- /v1/read ---
export type ReadRequest = Schemas['ReadRequest'];
export type ReadResponse = Schemas['ReadResponse'];
export type DocumentDto = Schemas['DocumentDto'];

// --- /v1/sources + /v1/documents ---
export type SourceDto = Schemas['SourceDto'];
export type SourcesResponse = Schemas['SourcesResponse'];
export type DocumentsResponse = Schemas['DocumentsResponse'];

// --- /v1/related ---
export type RelatedRequest = Schemas['RelatedRequest'];
export type RelatedResponse = Schemas['RelatedResponse'];
export type RelatedDocDto = Schemas['RelatedDocDto'];

// --- /v1/explain ---
export type ExplainRequest = Schemas['ExplainRequest'];
export type ExplainResponse = Schemas['ExplainResponse'];

// --- /v1/validate ---
export type ValidateRequest = Schemas['ValidateRequest'];
export type ValidateResponse = Schemas['ValidateResponse'];

// --- /v1/index + /v1/index_status ---
export type IndexRequest = Schemas['IndexRequest'];
export type IndexResponse = Schemas['IndexResponse'];
export type IndexStatusResponse = Schemas['IndexStatusResponse'];

// --- /v1/apply ---
export type ApplyRequest = Schemas['ApplyRequest'];
export type ApplyResponse = Schemas['ApplyResponse'];
/** One semantic mutation (internally tagged by `type`). */
export type CatOperationDto = Schemas['CatOperationDto'];

// --- /v1/build_context ---
export type BuildContextRequest = Schemas['BuildContextRequest'];
export type BuildContextResponse = Schemas['BuildContextResponse'];
export type ContextItemDto = Schemas['ContextItemDto'];
