/**
 * The Settings IPC boundary. The UI sees ordinary promises while the generated
 * Specta bindings keep the backend's closed, tagged error payloads intact.
 *
 * Library membership and diagnostic retention are backend-owned. This module
 * deliberately contains no browser fallback and no second settings store.
 */

import {
  commands,
  type CloudFilterInput,
  type CloudProvider,
  type CloudCatalogReady,
  type CloudSourceDto,
  type CloudSourceError,
  type CloudSourcesSnapshot,
  type CloseAndForgetLibrarySourceOutcome,
  type ConnectedCatdbState,
  type DiagnosticError,
  type DiagnosticEvent,
  type DiagnosticExport,
  type DiagnosticInfo,
  type DiagnosticLevel,
  type EntityQueryRequest_Deserialize,
  type LibraryEntityQueryResponse_Serialize,
  type LibraryError,
  type LibrarySnapshot,
  type LibrarySourceDto,
  type MobileEnterpriseError,
  type PickedLibrarySource,
  type WorkspaceGrant,
} from '../bindings';

/** Bounded semantic window rendered by the Library sidebar.
 *
 * Provider metadata is still read through CatDB, and selected document bodies
 * are fetched separately. A sidebar refresh must not turn into an eager export
 * of every record in a large connected source.
 */
export const LIBRARY_REFRESH_WINDOW = 1_000;
export const LIBRARY_LOGICAL_LIMIT = 100_000;

export interface LibraryQueryContinuation {
  queryId: string;
  cursor: string;
}

export type {
  CloudProvider,
  CloudFilterInput,
  CloudSourceDto,
  CloudSourcesSnapshot,
  ConnectedCatdbState,
  DiagnosticEvent,
  DiagnosticExport,
  DiagnosticInfo,
  DiagnosticLevel,
  LibrarySnapshot,
  LibrarySourceDto,
  CloseAndForgetLibrarySourceOutcome,
  PickedLibrarySource,
};

/** Generated native contract aliases retained for Settings component callers. */
export type CloudSourceView = CloudSourceDto;
export type CloudSourcesView = CloudSourcesSnapshot;

/** Preserve a backend's typed error for UI copy and tests without stringifying it. */
export class SettingsCommandError<T> extends Error {
  readonly detail: T;

  constructor(area: 'library' | 'diagnostics' | 'cloud' | 'connected-catdb', detail: T) {
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

function connectedCatdbFailure(error: MobileEnterpriseError): never {
  throw new SettingsCommandError('connected-catdb', error);
}

export async function connectedCatdbState(): Promise<ConnectedCatdbState> {
  const result = await commands.connectedCatdbState();
  if (result.status === 'error') return connectedCatdbFailure(result.error);
  return result.data;
}

export async function enrollConnectedCatdb(
  serverOrigin: string,
  bearerToken: string,
): Promise<ConnectedCatdbState> {
  const result = await commands.enrollConnectedCatdb(serverOrigin, bearerToken);
  if (result.status === 'error') return connectedCatdbFailure(result.error);
  return result.data;
}

export async function setConnectedCatdbSourceEnabled(
  connectionId: string,
  sourceId: string,
  enabled: boolean,
): Promise<ConnectedCatdbState> {
  const result = await commands.setConnectedCatdbSourceEnabled(
    connectionId,
    sourceId,
    enabled,
  );
  if (result.status === 'error') return connectedCatdbFailure(result.error);
  return result.data;
}

export async function disconnectConnectedCatdb(
  connectionId: string,
): Promise<ConnectedCatdbState> {
  const result = await commands.disconnectConnectedCatdb(connectionId);
  if (result.status === 'error') return connectedCatdbFailure(result.error);
  return result.data;
}

export interface SettingsBackend {
  listSources: () => Promise<LibrarySnapshot>;
  pickAndAddSource: () => Promise<PickedLibrarySource>;
  reconcileRestore: (sessionRoot: string | null) => Promise<LibrarySourceDto | null>;
  markActive: (sourceId: string) => Promise<LibrarySnapshot>;
  forgetSource: (sourceId: string) => Promise<LibrarySnapshot>;
  resolveLibraryDocumentEdit?: (sourceId: string, relativePath: string) => Promise<string>;
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
  queryLibraryDocuments?: (
    workspaceId: string,
    filters: CloudFilterInput[],
    fields: string[],
    requestId: string,
    sourceIds?: string[],
    continuation?: LibraryQueryContinuation,
  ) => Promise<LibraryEntityQueryResponse_Serialize>;
  queryConnectedCatdbDocuments?: (
    connectionId: string,
    filters: CloudFilterInput[],
    fields: string[],
    requestId: string,
  ) => Promise<LibraryEntityQueryResponse_Serialize>;
  cancelCloudDocumentQuery?: (requestId: string) => Promise<boolean>;
  connectedCatdbState?: () => Promise<ConnectedCatdbState>;
  enrollConnectedCatdb?: (
    serverOrigin: string,
    bearerToken: string,
  ) => Promise<ConnectedCatdbState>;
  setConnectedCatdbSourceEnabled?: (
    connectionId: string,
    sourceId: string,
    enabled: boolean,
  ) => Promise<ConnectedCatdbState>;
  disconnectConnectedCatdb?: (
    connectionId: string,
  ) => Promise<ConnectedCatdbState>;
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

export async function closeAndForgetLibrarySource(
  sourceId: string,
): Promise<CloseAndForgetLibrarySourceOutcome> {
  const result = await commands.closeAndForgetLibrarySource(sourceId);
  if (result.status === 'error') return libraryFailure(result.error);
  return result.data;
}

/** Resolve one semantic local path through the active identity-bound membership. */
export async function resolveLibraryDocumentEdit(
  sourceId: string,
  relativePath: string,
): Promise<string> {
  const result = await commands.resolveLibraryDocumentEdit(sourceId, relativePath);
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

/** Execute one immutable CatDB query over the selected Library memberships. */
export async function queryLibraryDocuments(
  workspaceId: string,
  filters: CloudFilterInput[],
  fields: string[],
  requestId: string,
  sourceIds?: string[],
  continuation?: LibraryQueryContinuation,
): Promise<LibraryEntityQueryResponse_Serialize> {
  const request: EntityQueryRequest_Deserialize = {
    workspace_id: workspaceId,
    catalog_revision_id: null,
    schema_version_id: null,
    entity: 'document.Document',
    select: fields,
    filters: filters.map((filter) => ({
      field: filter.field,
      operator: filter.operator,
      value: filter.value,
    })),
    order_by: [],
    source_ids: sourceIds ?? null,
    max_staleness_ms: null,
    require_complete: false,
    page_size: LIBRARY_REFRESH_WINDOW,
    limit: LIBRARY_LOGICAL_LIMIT,
    query_id: continuation?.queryId ?? null,
    cursor: continuation?.cursor ?? null,
  };
  const result = await commands.queryLibraryDocuments(requestId, request);
  if (result.status === 'error') return cloudFailure(result.error);
  return result.data;
}

/** Query the native-authorized Connected CatDB membership without exposing its bearer token. */
export async function queryConnectedCatdbDocuments(
  connectionId: string,
  filters: CloudFilterInput[],
  fields: string[],
  requestId: string,
): Promise<LibraryEntityQueryResponse_Serialize> {
  const request: EntityQueryRequest_Deserialize = {
    workspace_id: '00000000-0000-0000-0000-000000000000',
    catalog_revision_id: null,
    schema_version_id: null,
    entity: 'document.Document',
    select: fields,
    filters: filters.map((filter) => ({
      field: filter.field,
      operator: filter.operator,
      value: filter.value,
    })),
    order_by: [],
    source_ids: null,
    max_staleness_ms: null,
    require_complete: false,
    page_size: LIBRARY_REFRESH_WINDOW,
    limit: LIBRARY_REFRESH_WINDOW,
    cursor: null,
  };
  const result = await commands.queryConnectedLibraryDocuments(
    connectionId,
    requestId,
    request,
  );
  if (result.status === 'error') return connectedCatdbFailure(result.error);
  return result.data;
}

export async function cancelCloudDocumentQuery(requestId: string): Promise<boolean> {
  const [embedded, connected] = await Promise.all([
    commands.cancelCloudDocumentQuery(requestId),
    commands.cancelConnectedCatdbQuery(`${requestId}:connected-catdb`),
  ]);
  if (embedded.status === 'error') return cloudFailure(embedded.error);
  if (connected.status === 'error') return connectedCatdbFailure(connected.error);
  return embedded.data || connected.data;
}

export const settingsBackend: SettingsBackend = {
  listSources: listLibrarySources,
  pickAndAddSource: pickAndAddLibrarySource,
  reconcileRestore: reconcileLibraryRestore,
  markActive: markLibrarySourceActive,
  forgetSource: forgetLibrarySource,
  resolveLibraryDocumentEdit,
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
  queryLibraryDocuments,
  queryConnectedCatdbDocuments,
  cancelCloudDocumentQuery,
  connectedCatdbState,
  enrollConnectedCatdb,
  setConnectedCatdbSourceEnabled,
  disconnectConnectedCatdb,
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
