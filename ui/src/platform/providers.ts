import { readerBackend, unwrapNative, type NativeResult } from './backend';
import { providerRequestQueue } from './provider-request-queue';

export type NativeProviderKind = 'google_drive' | 'dropbox';
export interface ProviderErrorInfo { kind: string; operationId?: string | null; retryAfterMs?: number | null; error?: ProviderErrorInfo; diagnostic?: unknown }
export interface ProviderConnectionDiagnostic { phase: string; responseStatus: number | null; rejection: string | null }
const oauthPhases = new Set(['preparation', 'browser', 'callback', 'token_request', 'token_payload', 'refresh_token_missing', 'token_permissions', 'account_request', 'account_payload', 'grant']);
const oauthRejections = new Set(['invalid_client', 'invalid_grant', 'unauthorized_client', 'invalid_request', 'access_denied', 'other']);
/** Validate the closed native vocabulary; never render arbitrary error text. */
export function providerConnectionDiagnostic(cause: unknown): ProviderConnectionDiagnostic | null {
  if (!(cause instanceof NativeProviderError) || cause.detail.kind !== 'authorization_failed') return null;
  const value = cause.detail.diagnostic;
  if (!value || typeof value !== 'object' || !('phase' in value) || typeof value.phase !== 'string' || !oauthPhases.has(value.phase)
    || !('responseStatus' in value) || value.responseStatus !== null && (typeof value.responseStatus !== 'number' || !Number.isInteger(value.responseStatus) || value.responseStatus < 100 || value.responseStatus > 599)
    || !('rejection' in value) || value.rejection !== null && (typeof value.rejection !== 'string' || !oauthRejections.has(value.rejection))) return null;
  return { phase: value.phase, responseStatus: value.responseStatus, rejection: value.rejection };
}
function originalError(error: ProviderErrorInfo): ProviderErrorInfo {
  // Only one wrapper is produced by native OAuth; refuse recursive wrappers.
  return error.kind === 'authorization_failed' && error.error && error.error.kind !== 'authorization_failed' ? error.error : error;
}
/** The queued request was retired before any native invocation. */
export class ProviderRequestRetiredError extends Error {
  constructor() { super('The connection changed before creation. Try again from the current connection.'); }
}
export class NativeProviderError extends Error {
  constructor(readonly detail: ProviderErrorInfo) { super(providerErrorMessage(detail)); this.name = 'NativeProviderError'; }
}
export function providerErrorMessage(error: ProviderErrorInfo): string {
  error = originalError(error);
  switch (error.kind) {
    case 'unconfigured': return 'This provider is not configured in this build.';
    case 'busy': return 'Another provider operation is in progress. Try again after it finishes.';
    case 'service_unavailable': return 'The reader service is unavailable. Provider access has not been confirmed.';
    case 'unsupported': return 'This provider operation is unavailable on this platform or build.';
    case 'reconnect_required': case 'credential_unavailable': return 'Reconnect this source to restore provider access. Cached documents may still be available.';
    case 'unauthorized': return 'This source is paused or its authorization is no longer available.';
    case 'conflict': return 'The provider revision changed. Refresh and review the current document.';
    case 'unknown_write': return 'The save outcome is unknown. Reconcile it before making another save attempt.';
    case 'local_publication_pending': return 'The provider accepted the save. Local publication is still pending.';
    case 'timeout': return 'The operation timed out. Its outcome may still need reconciliation.';
    case 'cancelled': return 'The operation was cancelled.';
    case 'rate_limited': return 'The provider is rate limiting requests. Try again later.';
    case 'invalid': return 'The selected provider scope is invalid.';
    case 'too_large': return 'This provider operation exceeds the supported size or traversal limit. Choose a smaller folder or open the intended document from Sources.';
    default: return 'The provider operation is unavailable. Try a read of cached content or review the source connection.';
  }
}
/** A failed fresh sign-in does not mean an existing source was paused. */
export function providerConnectionErrorMessage(cause: unknown, provider: NativeProviderKind): string {
  const name = provider === 'google_drive' ? 'Google Drive' : 'Dropbox';
  if (!(cause instanceof NativeProviderError)) return `${name} could not connect. Try again.`;
  switch (originalError(cause.detail).kind) {
    case 'unauthorized': return `${name} sign-in or file selection was not accepted. Start the connection again to retry.`;
    case 'timeout': return `${name} sign-in took too long. Return to MDReader and start the connection again.`;
    case 'cancelled': return `${name} connection was cancelled. You can try again when you are ready.`;
    case 'invalid_response': return `${name} returned a response MDReader could not use. Start the connection again to retry.`;
    default: return cause.message;
  }
}
export interface ProviderAvailability { provider: NativeProviderKind; configured: boolean; supported: boolean; diagnostic: string | null }
export type ProviderScope = { kind: 'exact_documents'; count: number } | { kind: 'scoped_root'; recursive: boolean; path?: string | null } | { kind: 'drive_readonly_folder'; accountWide: boolean; folderId: string } | { kind: 'drive_folder'; folderId: string; recursive: boolean } | { kind: 'imported_legacy_broad' };
export type ProviderCoverage = { kind: 'never_indexed' } | { kind: 'partial'; reason: string } | { kind: 'metadata_complete_selected' | 'complete_selected'; observedAtMs: number };
export interface ProviderMetadataListing {
  knownDocuments: number;
  completeness: 'never_observed' | 'partial' | 'complete_selected';
  observedAtMs: number | null;
  /** Durable unfinished discovery; not evidence of a live network request. */
  refreshPending: boolean;
}
export interface ProviderSource {
  sourceId: string; readerSourceId: string | null; name: string; kind: NativeProviderKind; enabled: boolean;
  selectedScope: ProviderScope; capabilities: { read: boolean; list: boolean; changes: boolean; assets: boolean; guardedWrite: boolean; browseFolders?: boolean };
  health: 'ready' | 'paused' | 'reconnect_required' | 'unavailable'; coverage: ProviderCoverage; metadataListing?: ProviderMetadataListing;
}
export interface ProviderProjection { jobId: string; readerSourceId: string; path: string; revision: string }
export interface ProviderDocument {
  documentId: string; name: string; displayPath: string; modifiedAtMs: number | null; sizeBytes: number | null;
  cached: boolean; cachedObservedAtMs: number | null; admitted: boolean; projection: ProviderProjection | null;
}
export interface PendingProviderAdmission { attemptId: string; provider: NativeProviderKind; name: string; sourceId: string | null; phase: 'vault_pending' | 'admission_pending' }
export interface NativeProvidersSnapshot { availability: ProviderAvailability[]; sources: ProviderSource[]; pendingAdmissions: PendingProviderAdmission[]; error: ProviderErrorInfo | null }
export type ProviderAdmission = { state: 'admitted'; sourceId: string; enabled: boolean; replayed: boolean } | { state: 'pending'; attemptId: string; reason: ProviderErrorInfo };
export interface DropboxSelection { path: string; recursive: boolean }
export interface DriveFolderSelection { id: string; recursive?: boolean }
export interface ProviderFolders { operation: 'browse_folders'; folders: { id: string; name: string }[]; nextCursor: string | null }
export interface ProviderDocuments { operation: 'documents'; documents: ProviderDocument[]; offset: number; nextOffset: number | null }
/** The saved sign-in is kept only while another source still uses it. */
export interface ProviderDisconnect { credential: 'deleted' | 'shared'; revocation: 'revoked' | 'failed' | 'not_attempted' }
export interface ProviderRefresh { operation: 'refresh'; pages: number; metadataComplete: boolean }
export interface ProviderCreateResult {
  operation: 'create_document' | 'inspect_create'; operationId: string;
  state: 'absent' | 'prepared' | 'dispatched' | 'unknown' | 'accepted' | 'published' | 'conflict';
  projection: ProviderProjection | null;
}
export interface ProviderCreateIntent { sourceId: string; operationId: string; filename: string; markdown: string }
export interface ProviderReaderReference { sourceId: string; path: string; revision: string; title?: string; line?: number; heading?: string }
export interface ProviderLinkCandidate { sourceId: string; documentId: string; title: string }
export type ProviderLinkResolution =
  | { state: 'resolved'; candidate: ProviderLinkCandidate; projection: ProviderProjection; line: number; heading: string | null }
  | { state: 'selected_uncached'; candidate: ProviderLinkCandidate; heading: string | null }
  | { state: 'ambiguous'; candidates: ProviderLinkCandidate[]; heading: string | null }
  | { state: 'missing' }
  | { state: 'heading_missing'; candidate: ProviderLinkCandidate; heading: string };
export interface ProviderDocumentBody { sourceKind?: 'local' | 'provider' | 'capture'; operation: 'read_document'; markdown: string; sourceId: string; path: string; revision: string; lineStart: number; lineEnd: number }
export interface NativeProvidersBackend {
  state(): Promise<NativeProvidersSnapshot>;
  connect(provider: NativeProviderKind, selection: DropboxSelection | null, allowEditing?: boolean, driveFolder?: DriveFolderSelection): Promise<ProviderAdmission>;
  reconnect(sourceId: string, selection: DropboxSelection | null, allowEditing?: boolean, driveFolder?: DriveFolderSelection): Promise<ProviderAdmission>;
  browseFolders?(sourceId: string, folderId: string, cursor: string | null): Promise<ProviderFolders>;
  reconcile(attemptId: string): Promise<ProviderAdmission>;
  documents(sourceId: string, offset: number, limit: number): Promise<ProviderDocuments>;
  refresh(sourceId: string): Promise<ProviderRefresh>;
  createDocument?(intent: ProviderCreateIntent, stillCurrent?: () => boolean): Promise<ProviderCreateResult>;
  inspectCreate?(sourceId: string, operationId: string): Promise<ProviderCreateResult>;
  fetchCurrent(sourceId: string, documentId: string): Promise<ProviderProjection>;
  readCached(sourceId: string, documentId: string): Promise<ProviderProjection>;
  readDocument(reference: ProviderReaderReference): Promise<ProviderDocumentBody>;
  readCurrentDocument?(reference: ProviderReaderReference): Promise<ProviderDocumentBody>;
  resolveLink(reference: ProviderReaderReference, kind: 'relative' | 'wikilink', target: string): Promise<ProviderLinkResolution>;
  disable(sourceId: string): Promise<void>;
  /** Removes the source, its downloaded copies and its saved sign-in; revocation is best effort. */
  disconnect(sourceId: string): Promise<ProviderDisconnect>;
  /** Count of saves not yet confirmed as published (pending, uncertain or conflicted). Enabled sources only. */
  pendingChanges?(sourceId: string): Promise<number>;
}
const providers = () => readerBackend().providers;
/** Replies are trusted to the caller's declared projection, as before; only
 * the request side is checked against the generated DTOs. */
async function native<T>(call: () => Promise<NativeResult<unknown, unknown>>): Promise<T> {
  try { return await unwrapNative(call()) as T; }
  catch (cause) {
    if (cause && typeof cause === 'object' && 'kind' in cause && typeof cause.kind === 'string') throw new NativeProviderError(cause as ProviderErrorInfo);
    throw new NativeProviderError({ kind: 'unavailable' });
  }
}
export const nativeProvidersBackend: NativeProvidersBackend = {
  state: () => native(() => providers().nativeProviderStatus()),
  connect: (provider, selection, allowEditing = false, driveFolder) => native(() => providers().nativeProviderConnect(provider, selection, allowEditing, driveFolder ?? null)),
  reconnect: (sourceId, selection, allowEditing = false, driveFolder) => native(() => providers().nativeProviderReconnect(sourceId, selection, allowEditing, driveFolder ?? null)),
  browseFolders: (sourceId, folderId, cursor) => native(() => providers().nativeProviderRequest({ operation: 'browse_folders', sourceId, folderId, cursor })),
  reconcile: (attemptId) => native(() => providers().nativeProviderReconcile(attemptId)),
  documents: (sourceId, offset, limit) => native(() => providers().nativeProviderRequest({ operation: 'documents', sourceId, offset, limit })),
  refresh: (sourceId) => providerRequestQueue.run('metadata', () => native(() => providers().nativeProviderRequest({ operation: 'refresh', sourceId }))),
  createDocument: (intent, stillCurrent) => providerRequestQueue.run('foreground', () => {
    if (stillCurrent && !stillCurrent()) throw new ProviderRequestRetiredError();
    return native(() => providers().nativeProviderRequest({ operation: 'create_document', ...intent }));
  }),
  inspectCreate: (sourceId, operationId) => providerRequestQueue.run('foreground', () => native(() => providers().nativeProviderRequest({ operation: 'inspect_create', sourceId, operationId }))),
  async fetchCurrent(sourceId, documentId) { const result = await providerRequestQueue.run('foreground', () => native<{ operation: 'fetch_current'; projection: ProviderProjection }>(() => providers().nativeProviderRequest({ operation: 'fetch_current', sourceId, documentId }))); return result.projection; },
  async readCached(sourceId, documentId) { const result = await native<{ operation: 'read_cached'; projection: ProviderProjection }>(() => providers().nativeProviderRequest({ operation: 'read_cached', sourceId, documentId })); return result.projection; },
  async readCurrentDocument(reference) {
    const result = await native<Omit<ProviderDocumentBody, 'operation'> & { operation: 'read_current_document' }>(() => providers().nativeProviderRequest({ operation: 'read_current_document', sourceId: reference.sourceId, path: reference.path, revision: reference.revision }));
    if (result.operation !== 'read_current_document') throw new NativeProviderError({ kind: 'invalid' });
    return { ...result, operation: 'read_document' };
  },
  readDocument: (reference) => native(() => providers().nativeProviderRequest({ operation: 'read_document', sourceId: reference.sourceId, path: reference.path, revision: reference.revision })),
  async resolveLink(reference, kind, target) { const result = await native<{ operation: 'resolve_link'; resolution: ProviderLinkResolution }>(() => providers().nativeProviderRequest({ operation: 'resolve_link', sourceId: reference.sourceId, path: reference.path, revision: reference.revision, kind, target })); return result.resolution; },
  async disable(sourceId) { await native(() => providers().nativeProviderRequest({ operation: 'disable', sourceId })); },
  async pendingChanges(sourceId) {
    const result = await native<{ operation: 'mutations'; mutations: unknown[] }>(() => providers().nativeProviderRequest({ operation: 'mutations', sourceId }));
    return result.mutations.length;
  },
  async disconnect(sourceId) {
    const result = await native<{ operation: 'disconnect' } & ProviderDisconnect>(() => providers().nativeProviderRequest({ operation: 'disconnect', sourceId }));
    return { credential: result.credential, revocation: result.revocation };
  },
};
