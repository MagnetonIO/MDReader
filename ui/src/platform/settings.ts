/**
 * The Settings IPC boundary. The UI sees ordinary promises while the generated
 * Specta bindings keep the backend's closed, tagged error payloads intact.
 *
 * Library membership and diagnostic retention are backend-owned. This module
 * deliberately contains no browser fallback and no second settings store.
 */

import {
  commands,
  type CloudDocumentRow,
  type CloudDocumentQueryResponse,
  type CloudFilterInput,
  type CloudProvider,
  type CloudCatalogReady,
  type CloudSourceDto,
  type CloudSourceError,
  type CloudSourcesSnapshot,
  type DiagnosticError,
  type DiagnosticEvent,
  type DiagnosticExport,
  type DiagnosticInfo,
  type DiagnosticLevel,
  type EntityQueryResponse,
  type EntityQueryRequest_Deserialize,
  type LibraryError,
  type LibrarySnapshot,
  type LibrarySourceDto,
  type PickedLibrarySource,
  type WorkspaceGrant,
} from '../bindings';

export type {
  CloudProvider,
  CloudDocumentRow,
  CloudDocumentQueryResponse,
  CloudFilterInput,
  CloudSourceDto,
  CloudSourcesSnapshot,
  DiagnosticEvent,
  DiagnosticExport,
  DiagnosticInfo,
  DiagnosticLevel,
  LibrarySnapshot,
  LibrarySourceDto,
  PickedLibrarySource,
};

/** Generated native contract aliases retained for Settings component callers. */
export type CloudSourceView = CloudSourceDto;
export type CloudSourcesView = CloudSourcesSnapshot;

/** Preserve a backend's typed error for UI copy and tests without stringifying it. */
export class SettingsCommandError<T> extends Error {
  readonly detail: T;

  constructor(area: 'library' | 'diagnostics' | 'cloud', detail: T) {
    super(`${area} command failed`);
    this.name = 'SettingsCommandError';
    this.detail = detail;
  }
}

function libraryFailure(error: LibraryError): never {
  throw new SettingsCommandError('library', error);
}

function diagnosticFailure(error: DiagnosticError): never {
  throw new SettingsCommandError('diagnostics', error);
}

function cloudFailure(error: CloudSourceError): never {
  throw new SettingsCommandError('cloud', error);
}

export interface SettingsBackend {
  listSources: () => Promise<LibrarySnapshot>;
  pickAndAddSource: () => Promise<PickedLibrarySource>;
  reconcileRestore: (sessionRoot: string | null) => Promise<LibrarySourceDto | null>;
  markActive: (sourceId: string) => Promise<LibrarySnapshot>;
  forgetSource: (sourceId: string) => Promise<LibrarySnapshot>;
  diagnosticsInfo: () => Promise<DiagnosticInfo>;
  setDiagnosticLevel: (level: DiagnosticLevel) => Promise<DiagnosticInfo>;
  recordDiagnosticEvent: (event: DiagnosticEvent) => Promise<void>;
  revealDiagnostics: () => Promise<void>;
  exportDiagnostics: () => Promise<DiagnosticExport>;
  clearDiagnostics: (confirmed: boolean) => Promise<DiagnosticInfo>;
  /**
   * Present only when the native application supplies its registered OAuth
   * apps and credential-vault commands. Browser/test shells omit the entire
   * cloud-account surface instead of displaying controls that cannot work.
   */
  listCloudSources?: () => Promise<CloudSourcesView>;
  prepareCloudCatalog?: () => Promise<CloudCatalogReady>;
  connectCloudSource?: (provider: CloudProvider) => Promise<CloudSourcesView>;
  setCloudSourceEnabled?: (
    provider: CloudProvider,
    enabled: boolean,
  ) => Promise<CloudSourcesView>;
  disconnectCloudSource?: (provider: CloudProvider) => Promise<CloudSourcesView>;
  queryCloudDocuments?: (
    provider: CloudProvider,
    filters: CloudFilterInput[],
    fields: string[],
    requestId: string,
  ) => Promise<CloudDocumentQueryResponse>;
  queryCloudEntity?: (
    provider: CloudProvider,
    filters: CloudFilterInput[],
    fields: string[],
    requestId: string,
  ) => Promise<EntityQueryResponse>;
  cancelCloudDocumentQuery?: (requestId: string) => Promise<boolean>;
}

export async function listLibrarySources(): Promise<LibrarySnapshot> {
  const result = await commands.listLibrarySources();
  if (result.status === 'error') return libraryFailure(result.error);
  return result.data;
}

export async function pickAndAddLibrarySource(): Promise<PickedLibrarySource> {
  const result = await commands.pickAndAddLibrarySource();
  if (result.status === 'error') return libraryFailure(result.error);
  return result.data;
}

export async function reconcileLibraryRestore(
  sessionRoot: string | null,
): Promise<LibrarySourceDto | null> {
  const result = await commands.reconcileLibraryRestore(sessionRoot);
  if (result.status === 'error') return libraryFailure(result.error);
  return result.data;
}

/** Arm the watcher and grant the exact native registry path for `sourceId`. */
export async function watchLibrarySource(sourceId: string): Promise<WorkspaceGrant> {
  const result = await commands.watchLibrarySource(sourceId);
  if (result.status === 'error') return libraryFailure(result.error);
  return result.data;
}

/** Grant a known source without a watcher for the existing degraded-open path. */
export async function grantLibrarySource(sourceId: string): Promise<string> {
  const result = await commands.grantLibrarySource(sourceId);
  if (result.status === 'error') return libraryFailure(result.error);
  return result.data;
}

export async function markLibrarySourceActive(sourceId: string): Promise<LibrarySnapshot> {
  const result = await commands.markLibrarySourceActive(sourceId);
  if (result.status === 'error') return libraryFailure(result.error);
  return result.data;
}

export async function forgetLibrarySource(sourceId: string): Promise<LibrarySnapshot> {
  const result = await commands.forgetLibrarySource(sourceId);
  if (result.status === 'error') return libraryFailure(result.error);
  return result.data;
}

export async function getDiagnosticsInfo(): Promise<DiagnosticInfo> {
  const result = await commands.diagnosticsInfo();
  if (result.status === 'error') return diagnosticFailure(result.error);
  return result.data;
}

export async function setDiagnosticsLevel(level: DiagnosticLevel): Promise<DiagnosticInfo> {
  const result = await commands.setDiagnosticsLevel(level);
  if (result.status === 'error') return diagnosticFailure(result.error);
  return result.data;
}

export async function recordDiagnosticEvent(event: DiagnosticEvent): Promise<void> {
  const result = await commands.recordDiagnosticEvent(event);
  if (result.status === 'error') return diagnosticFailure(result.error);
}

export async function revealDiagnostics(): Promise<void> {
  const result = await commands.revealDiagnostics();
  if (result.status === 'error') return diagnosticFailure(result.error);
}

export async function exportDiagnostics(): Promise<DiagnosticExport> {
  const result = await commands.exportDiagnostics();
  if (result.status === 'error') return diagnosticFailure(result.error);
  return result.data;
}

export async function clearDiagnostics(confirmed: boolean): Promise<DiagnosticInfo> {
  const result = await commands.clearDiagnostics(confirmed);
  if (result.status === 'error') return diagnosticFailure(result.error);
  return result.data;
}

export async function listCloudSources(): Promise<CloudSourcesSnapshot> {
  const result = await commands.listCloudSources();
  if (result.status === 'error') return cloudFailure(result.error);
  return result.data;
}

export async function prepareCloudCatalog(): Promise<CloudCatalogReady> {
  const result = await commands.prepareCloudCatalog();
  if (result.status === 'error') return cloudFailure(result.error);
  return result.data;
}

export async function connectCloudSource(
  provider: CloudProvider,
): Promise<CloudSourcesSnapshot> {
  const result = await commands.connectCloudSource(provider);
  if (result.status === 'error') return cloudFailure(result.error);
  return result.data;
}

export async function setCloudSourceEnabled(
  provider: CloudProvider,
  enabled: boolean,
): Promise<CloudSourcesSnapshot> {
  const result = await commands.setCloudSourceEnabled(provider, enabled);
  if (result.status === 'error') return cloudFailure(result.error);
  return result.data;
}

export async function disconnectCloudSource(
  provider: CloudProvider,
): Promise<CloudSourcesSnapshot> {
  const result = await commands.disconnectCloudSource(provider);
  if (result.status === 'error') return cloudFailure(result.error);
  return result.data;
}

export async function queryCloudDocuments(
  provider: CloudProvider,
  filters: CloudFilterInput[],
  fields: string[],
  requestId: string,
): Promise<CloudDocumentQueryResponse> {
  const result = await commands.queryCloudDocuments(requestId, provider, filters, fields);
  if (result.status === 'error') return cloudFailure(result.error);
  return result.data;
}

export async function queryCloudEntity(
  provider: CloudProvider,
  filters: CloudFilterInput[],
  fields: string[],
  requestId: string,
): Promise<EntityQueryResponse> {
  const catalog = await prepareCloudCatalog();
  const request: EntityQueryRequest_Deserialize = {
    workspace_id: catalog.workspace_id,
    catalog_revision_id: catalog.catalog_revision_id,
    schema_version_id: catalog.schema_version_id,
    entity: 'library.Document',
    select: fields,
    filters: filters.map((filter) => ({
      field: filter.field,
      operator: filter.operator,
      value: filter.value,
    })),
    order_by: [],
    source_ids: null,
    max_staleness_ms: null,
    require_complete: true,
    page_size: 1_000,
    limit: 1_000,
    cursor: null,
  };
  const result = await commands.entityQuery(requestId, provider, request);
  if (result.status === 'error') return cloudFailure(result.error);
  return result.data;
}

export async function cancelCloudDocumentQuery(requestId: string): Promise<boolean> {
  const result = await commands.cancelCloudDocumentQuery(requestId);
  if (result.status === 'error') return cloudFailure(result.error);
  return result.data;
}

export const settingsBackend: SettingsBackend = {
  listSources: listLibrarySources,
  pickAndAddSource: pickAndAddLibrarySource,
  reconcileRestore: reconcileLibraryRestore,
  markActive: markLibrarySourceActive,
  forgetSource: forgetLibrarySource,
  diagnosticsInfo: getDiagnosticsInfo,
  setDiagnosticLevel: setDiagnosticsLevel,
  recordDiagnosticEvent,
  revealDiagnostics,
  exportDiagnostics,
  clearDiagnostics,
  listCloudSources,
  prepareCloudCatalog,
  connectCloudSource,
  setCloudSourceEnabled,
  disconnectCloudSource,
  queryCloudDocuments,
  queryCloudEntity,
  cancelCloudDocumentQuery,
};

/**
 * Logging is observational only. Intentionally return `void`: callers cannot
 * await this helper and therefore cannot accidentally gate a switch/import/read.
 */
export function recordDiagnosticEventNonBlocking(
  event: DiagnosticEvent,
  write: (event: DiagnosticEvent) => Promise<void> = recordDiagnosticEvent,
): void {
  void write(event).catch(() => {
    /* Local diagnostics must never become a product failure. */
  });
}
