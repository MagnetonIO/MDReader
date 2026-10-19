/**
 * The Settings IPC boundary. The UI sees ordinary promises while the generated
 * Specta bindings keep the backend's closed, tagged error payloads intact.
 *
 * Library membership and diagnostic retention are backend-owned. This module
 * deliberately contains no browser fallback and no second settings store.
 */

import type {
  CloseAndForgetLibrarySourceOutcome,
  DiagnosticError, DiagnosticEvent, DiagnosticExport,
  DiagnosticInfo, DiagnosticLevel,
  LibraryError, LibrarySnapshot, LibrarySourceDto,
  PickedLibrarySource, WorkspaceGrant,
} from '../bindings';
import { readerBackend } from './backend';

export type {
  DiagnosticEvent, DiagnosticExport, DiagnosticInfo, DiagnosticLevel,
  LibrarySnapshot, LibrarySourceDto, CloseAndForgetLibrarySourceOutcome,
  PickedLibrarySource,
};

/** Preserve a backend's typed error for UI copy and tests without stringifying it. */
export class SettingsCommandError<T> extends Error {
  readonly detail: T;

  constructor(
    area: 'library' | 'diagnostics',
    detail: T,
  ) {
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

}

export async function listLibrarySources(): Promise<LibrarySnapshot> {
  const result = await readerBackend().library.listLibrarySources();
  if (result.status === 'error') return libraryFailure(result.error);
  return result.data;
}

export async function pickAndAddLibrarySource(): Promise<PickedLibrarySource> {
  const result = await readerBackend().library.pickAndAddLibrarySource();
  if (result.status === 'error') return libraryFailure(result.error);
  return result.data;
}

export async function reconcileLibraryRestore(
  sessionRoot: string | null,
): Promise<LibrarySourceDto | null> {
  const result = await readerBackend().library.reconcileLibraryRestore(sessionRoot);
  if (result.status === 'error') return libraryFailure(result.error);
  return result.data;
}

/** Arm the watcher and grant the exact native registry path for `sourceId`. */
export async function watchLibrarySource(sourceId: string): Promise<WorkspaceGrant> {
  const result = await readerBackend().library.watchLibrarySource(sourceId);
  if (result.status === 'error') return libraryFailure(result.error);
  return result.data;
}

/** Grant a known source without a watcher for the existing degraded-open path. */
export async function grantLibrarySource(sourceId: string): Promise<string> {
  const result = await readerBackend().library.grantLibrarySource(sourceId);
  if (result.status === 'error') return libraryFailure(result.error);
  return result.data;
}

export async function markLibrarySourceActive(sourceId: string): Promise<LibrarySnapshot> {
  const result = await readerBackend().library.markLibrarySourceActive(sourceId);
  if (result.status === 'error') return libraryFailure(result.error);
  return result.data;
}

export async function forgetLibrarySource(sourceId: string): Promise<LibrarySnapshot> {
  const result = await readerBackend().library.forgetLibrarySource(sourceId);
  if (result.status === 'error') return libraryFailure(result.error);
  return result.data;
}

export async function closeAndForgetLibrarySource(
  sourceId: string,
): Promise<CloseAndForgetLibrarySourceOutcome> {
  const result = await readerBackend().library.closeAndForgetLibrarySource(sourceId);
  if (result.status === 'error') return libraryFailure(result.error);
  return result.data;
}

export async function getDiagnosticsInfo(): Promise<DiagnosticInfo> {
  const result = await readerBackend().diagnostics.diagnosticsInfo();
  if (result.status === 'error') return diagnosticFailure(result.error);
  return result.data;
}

export async function setDiagnosticsLevel(level: DiagnosticLevel): Promise<DiagnosticInfo> {
  const result = await readerBackend().diagnostics.setDiagnosticsLevel(level);
  if (result.status === 'error') return diagnosticFailure(result.error);
  return result.data;
}

export async function recordDiagnosticEvent(event: DiagnosticEvent): Promise<void> {
  const result = await readerBackend().diagnostics.recordDiagnosticEvent(event);
  if (result.status === 'error') return diagnosticFailure(result.error);
}

export async function revealDiagnostics(): Promise<void> {
  const result = await readerBackend().diagnostics.revealDiagnostics();
  if (result.status === 'error') return diagnosticFailure(result.error);
}

export async function exportDiagnostics(): Promise<DiagnosticExport> {
  const result = await readerBackend().diagnostics.exportDiagnostics();
  if (result.status === 'error') return diagnosticFailure(result.error);
  return result.data;
}

export async function clearDiagnostics(confirmed: boolean): Promise<DiagnosticInfo> {
  const result = await readerBackend().diagnostics.clearDiagnostics(confirmed);
  if (result.status === 'error') return diagnosticFailure(result.error);
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
