import { useEffect, useRef, useState } from 'react';
import {
  NativeProviderError, providerErrorMessage, providerConnectionErrorMessage, providerConnectionDiagnostic, type ProviderConnectionDiagnostic,
  type NativeProvidersBackend, type NativeProvidersSnapshot, type ProviderReaderReference,
  type NativeProviderKind, type ProviderAdmission, type ProviderDocuments, type ProviderSource,
  type DropboxSelection, type DriveFolderSelection, type ProviderDocument,
} from '../../platform/providers';
import { disconnectNotice } from './source-presentation';

const message = (cause: unknown) => cause instanceof NativeProviderError ? cause.message : 'The provider operation could not be completed.';
export function validDropboxPath(path: string): boolean {
  return path.length > 0 && path.length <= 4096 && path.startsWith('/') && !path.includes('\\') && !Array.from(path).some((character) => { const code = character.charCodeAt(0); return code < 32 || code >= 127 && code <= 159; })
    && !path.split('/').some((part) => part === '.' || part === '..') && (path === '/' || !path.endsWith('/'));
}

export interface NativeProvidersOptions {
  backend: NativeProvidersBackend;
  profileKey: string;
  /** Status is read only once the panel is first shown. */
  active: boolean;
  allowEditing: boolean;
  onOpen?: (reference: ProviderReaderReference) => void | Promise<void>;
  onSourcesChanged?: () => void;
}

/** Lifecycle epochs discard late replies after a profile change; one action runs at a time. */
export function useNativeProviders({ backend, profileKey, active, allowEditing, onOpen, onSourcesChanged }: NativeProvidersOptions) {
  const [opened, setOpened] = useState(active);
  if (active && !opened) setOpened(true);
  const [snapshot, setSnapshot] = useState<NativeProvidersSnapshot | null>(null);
  const [selected, setSelected] = useState<string | null>(null);
  const [browseGeneration, setBrowseGeneration] = useState(0);
  const [page, setPage] = useState<ProviderDocuments | null>(null);
  const [loading, setLoading] = useState(false);
  const [documentsLoading, setDocumentsLoading] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [connectionDiagnostic, setConnectionDiagnostic] = useState<ProviderConnectionDiagnostic | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [adding, setAdding] = useState<NativeProviderKind | null>(null);
  const [dropboxPath, setDropboxPath] = useState('/');
  const [recursive, setRecursive] = useState(false);
  const [dropboxEditing, setDropboxEditing] = useState(false);
  const [driveRecursive, setDriveRecursive] = useState(true);
  const [driveEditing, setDriveEditing] = useState(false);
  const [folderSource, setFolderSource] = useState<string | null>(null);
  const lifecycle = useRef(0);
  const loadedProfile = useRef<string | null>(null);
  const documentRequest = useRef(0);
  const action = useRef(0);
  const selection = useRef<string | null>(null);
  selection.current = selected;
  useEffect(() => {
    const epoch = ++lifecycle.current;
    loadedProfile.current = null;
    documentRequest.current += 1;
    action.current += 1;
    setDropboxEditing(false); setDriveEditing(false); setDriveRecursive(true); setFolderSource(null); setAdding(null); setSnapshot(null); setSelected(null); setPage(null); setError(null); setConnectionDiagnostic(null); setNotice(null); setBusy(false); setDocumentsLoading(false);
    if (!opened) { setLoading(false); return; }
    setLoading(true);
    void backend.state().then((value) => { if (lifecycle.current === epoch) { loadedProfile.current = profileKey; setSnapshot(value); } })
      .catch((cause: unknown) => { if (lifecycle.current === epoch) setError(message(cause)); })
      .finally(() => { if (lifecycle.current === epoch) setLoading(false); });
    return () => { lifecycle.current += 1; documentRequest.current += 1; action.current += 1; };
  }, [backend, opened, profileKey]);
  useEffect(() => {
    const request = ++documentRequest.current;
    const epoch = lifecycle.current;
    setPage(null); setDocumentsLoading(false);
    if (!selected || !opened || loadedProfile.current !== profileKey) return;
    setDocumentsLoading(true);
    void backend.documents(selected, 0, 50).then((value) => {
      if (documentRequest.current === request && lifecycle.current === epoch) setPage(value);
    }).catch((cause: unknown) => {
      if (documentRequest.current === request && lifecycle.current === epoch) setError(message(cause));
    }).finally(() => {
      if (documentRequest.current === request && lifecycle.current === epoch) setDocumentsLoading(false);
    });
    return () => { documentRequest.current += 1; };
  }, [backend, selected, profileKey, browseGeneration]);
  const current = snapshot?.sources.find((source) => source.sourceId === selected) ?? null;
  const canConnect = (kind: NativeProviderKind) => !snapshot?.error && (snapshot?.availability.some((value) => value.provider === kind && value.configured && value.supported) ?? false);
  const dropboxSelection = (): DropboxSelection => ({ path: dropboxPath, recursive });
  async function updateSnapshot(epoch: number) {
    const value = await backend.state();
    if (lifecycle.current === epoch) {
      setSnapshot(value);
      if (selection.current && !value.sources.some((source) => source.sourceId === selection.current && source.enabled)) {
        documentRequest.current += 1; setSelected(null); setPage(null);
      }
    }
  }
  async function run(work: (epoch: number) => Promise<void>, connection?: NativeProviderKind) {
    if (busy) return;
    const epoch = lifecycle.current;
    const id = ++action.current;
    setBusy(true); setError(null); setConnectionDiagnostic(null);
    setNotice(connection === 'google_drive' ? 'Finish the Google permission request, then return to MDReader.'
      : connection === 'dropbox' ? 'Finish signing in to Dropbox in your browser, then return here.' : null);
    try { await work(epoch); }
    catch (cause) {
      if (lifecycle.current === epoch && action.current === id) {
        setNotice(null);
        setConnectionDiagnostic(connection ? providerConnectionDiagnostic(cause) : null);
        setError(connection ? providerConnectionErrorMessage(cause, connection) : message(cause));
      }
    }
    finally { if (lifecycle.current === epoch && action.current === id) setBusy(false); }
  }
  async function acceptAdmission(result: ProviderAdmission, epoch: number) {
    if (lifecycle.current !== epoch) return;
    if (result.state === 'pending') {
      setNotice('The connection outcome is pending. Use Reconcile connection to check it without starting OAuth again.');
      setError(providerErrorMessage(result.reason));
    } else {
      setAdding(null);
      setNotice(result.enabled ? 'The selected source is connected. Refresh its file list to browse documents.' : 'This source remains paused. Reconnect it explicitly to grant access again.');
      onSourcesChanged?.();
    }
    await updateSnapshot(epoch);
  }
  function connect(kind: NativeProviderKind) {
    if (!canConnect(kind) || kind === 'dropbox' && !validDropboxPath(dropboxPath)) return;
    if (kind === 'google_drive') { connectDriveFolder({ id: 'root' }); return; }
    void run(async (epoch) => acceptAdmission(await backend.connect(kind, dropboxSelection(), allowEditing && dropboxEditing), epoch), kind);
  }
  function connectDriveFolder(folder: DriveFolderSelection) {
    if (!canConnect('google_drive')) return;
    void run(async (epoch) => {
      await acceptAdmission(await backend.connect('google_drive', null, allowEditing && driveEditing, { id: folder.id, recursive: driveRecursive }), epoch);
      if (lifecycle.current === epoch) setFolderSource(null);
    }, 'google_drive');
  }
  function connectGoogleFiles() {
    if (canConnect('google_drive')) void run(async (epoch) => acceptAdmission(await backend.connect('google_drive', null, false), epoch), 'google_drive');
  }
  function reconnect(source: ProviderSource) {
    if (!canConnect(source.kind)) return;
    if (source.kind === 'google_drive' && source.selectedScope.kind === 'drive_readonly_folder') {
      const folder = { id: source.selectedScope.folderId };
      void run(async (epoch) => acceptAdmission(await backend.reconnect(source.sourceId, null, false, folder), epoch), source.kind);
      return;
    }
    if (source.kind === 'google_drive' && source.selectedScope.kind === 'drive_folder') {
      const folder = { id: source.selectedScope.folderId, recursive: source.selectedScope.recursive };
      void run(async epoch => acceptAdmission(await backend.reconnect(source.sourceId, null, allowEditing && source.capabilities.guardedWrite, folder), epoch), source.kind);
      return;
    }
    if (source.kind === 'dropbox' && source.selectedScope.kind === 'scoped_root' && source.selectedScope.path) {
      const retained = { path: source.selectedScope.path, recursive: source.selectedScope.recursive };
      void run(async epoch => acceptAdmission(await backend.reconnect(source.sourceId, retained, allowEditing && source.capabilities.guardedWrite), epoch), source.kind);
      return;
    }
    // Older scopes without a retained folder path require explicit fresh selection.
    if (source.kind === 'dropbox') {
      setError('Use Add source to choose the Dropbox folder again. The saved folder path is unavailable.');
      return;
    }
    void run(async epoch => acceptAdmission(await backend.reconnect(source.sourceId, null, false), epoch), source.kind);
  }
  function pause(source: ProviderSource) {
    void run(async (epoch) => {
      await backend.disable(source.sourceId);
      if (lifecycle.current !== epoch) return;
      setNotice('This source is paused. Its downloaded copies are no longer accessible.');
      onSourcesChanged?.();
      await updateSnapshot(epoch);
    });
  }
  function disconnect(source: ProviderSource) {
    void run(async (epoch) => {
      const result = await backend.disconnect(source.sourceId);
      if (lifecycle.current !== epoch) return;
      if (folderSource === source.sourceId) setFolderSource(null);
      setNotice(disconnectNotice(source, result));
      onSourcesChanged?.();
      await updateSnapshot(epoch);
    });
  }
  function refresh(source: ProviderSource) {
    void run(async (epoch) => {
      const result = await backend.refresh(source.sourceId);
      if (lifecycle.current !== epoch) return;
      setNotice(result.metadataComplete ? 'File list refreshed. Open a file to download it.' : 'More files remain to be found. Open this source in the library to continue loading.');
      onSourcesChanged?.();
      await updateSnapshot(epoch);
      if (lifecycle.current !== epoch || selection.current !== source.sourceId) return;
      const request = ++documentRequest.current;
      const documents = await backend.documents(source.sourceId, 0, 50);
      if (lifecycle.current === epoch && documentRequest.current === request && selection.current === source.sourceId) setPage(documents);
    });
  }
  function open(source: ProviderSource, document: ProviderDocument, cached: boolean) {
    if (!onOpen) return;
    void run(async (epoch) => {
      const projection = cached ? await backend.readCached(source.sourceId, document.documentId) : await backend.fetchCurrent(source.sourceId, document.documentId);
      if (lifecycle.current !== epoch || selection.current !== source.sourceId) return;
      if (!cached) onSourcesChanged?.();
      await onOpen({ sourceId: projection.readerSourceId, path: projection.path, revision: projection.revision, title: document.name, line: 1 });
      if (lifecycle.current === epoch) setNotice(cached ? 'Opened the exact saved copy. Its check time does not prove it matches the current file.' : 'Downloaded and opened the latest published version. Indexing may still be in progress.');
    });
  }
  function more() {
    if (!current || !page || page.nextOffset === null || documentsLoading || busy) return;
    const source = current.sourceId; const offset = page.nextOffset; const epoch = lifecycle.current; const request = ++documentRequest.current;
    setDocumentsLoading(true);
    void backend.documents(source, offset, 50).then((value) => {
      if (lifecycle.current === epoch && documentRequest.current === request && selection.current === source) setPage((previous) => ({ ...value, documents: [...(previous?.documents ?? []), ...value.documents] }));
    }).catch((cause: unknown) => { if (lifecycle.current === epoch && documentRequest.current === request) setError(message(cause)); })
      .finally(() => { if (lifecycle.current === epoch && documentRequest.current === request) setDocumentsLoading(false); });
  }
  function browse(source: ProviderSource) {
    setError(null); setNotice(null); setSelected(source.sourceId); setBrowseGeneration((value) => value + 1);
  }
  function closeDocuments() { documentRequest.current += 1; setSelected(null); setPage(null); }
  /** Only an admitted My Drive root source with native folder browsing can start a narrower folder connection. */
  function canChooseDriveFolder(source: ProviderSource): boolean {
    const scope = source.selectedScope;
    return source.enabled && source.kind === 'google_drive' && (scope.kind === 'drive_folder' || scope.kind === 'drive_readonly_folder' && scope.accountWide)
      && scope.folderId === 'root' && source.capabilities.browseFolders === true && !!backend.browseFolders;
  }
  return {
    snapshot, loading, busy, error, setError, connectionDiagnostic, notice, current, page, documentsLoading,
    adding, setAdding, folderSource, setFolderSource, canConnect, canChooseDriveFolder,
    form: { dropboxPath, pathValid: validDropboxPath(dropboxPath), recursive, dropboxEditing, driveRecursive, driveEditing,
      setDropboxPath, setRecursive, setDropboxEditing, setDriveRecursive, setDriveEditing },
    refreshStatus: () => void run(updateSnapshot),
    reconcile: (attemptId: string) => void run(async (epoch) => acceptAdmission(await backend.reconcile(attemptId), epoch)),
    connect, connectDriveFolder, connectGoogleFiles, reconnect, refresh, open, more, browse, closeDocuments, pause, disconnect,
  };
}

export type NativeProvidersController = ReturnType<typeof useNativeProviders>;
