import { useEffect, useRef, useState } from 'react';
import { NativeProviderError, type DriveFolderSelection, type NativeProvidersBackend, type ProviderFolders } from '../../platform/providers';

type Folder = { id: string; name: string };
function checked(value: ProviderFolders): ProviderFolders {
  if (value.operation !== 'browse_folders' || !Array.isArray(value.folders) || value.folders.length > 256 ||
    value.folders.some(folder => typeof folder.id !== 'string' || !/^[A-Za-z0-9_-]{1,256}$/.test(folder.id) ||
      typeof folder.name !== 'string' || new TextEncoder().encode(folder.name).length > 4096) ||
    new Set(value.folders.map(folder => folder.id)).size !== value.folders.length ||
    value.nextCursor !== null && (typeof value.nextCursor !== 'string' || value.nextCursor.length > 65536 || !/^[\x20-\x7e]+$/.test(value.nextCursor)))
    throw new Error('The Drive folder page could not be verified.');
  return value;
}

/** Folder IDs come only from an admitted broad read-only source. Choosing one starts a fresh connection. */
export function DriveFolderBrowser({ sourceId, backend, busy, onChoose }: {
  sourceId: string; backend: NativeProvidersBackend; busy: boolean;
  onChoose(folder: DriveFolderSelection): void;
}) {
  const [trail, setTrail] = useState<Folder[]>([{ id: 'root', name: 'Drive' }]);
  const folder = trail[trail.length - 1]!;
  const [page, setPage] = useState<ProviderFolders | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const epoch = useRef(0);
  async function load(cursor: string | null, more: boolean) {
    if (!backend.browseFolders) return;
    const request = ++epoch.current;
    setLoading(true); setError(null);
    if (!more) setPage(null);
    try {
      const value = checked(await backend.browseFolders(sourceId, folder.id, cursor));
      if (request !== epoch.current) return;
      setPage(previous => {
        if (!more || !previous) return value;
        const combined = new Map(previous.folders.map(item => [item.id, item]));
        for (const item of value.folders) combined.set(item.id, item);
        return { ...value, folders: [...combined.values()].slice(0, 1000) };
      });
    } catch (cause) {
      if (request === epoch.current) setError(cause instanceof NativeProviderError ? cause.message : 'The Drive folder page could not be loaded.');
    } finally { if (request === epoch.current) setLoading(false); }
  }
  useEffect(() => {
    void load(null, false);
    return () => { epoch.current++; };
  }, [backend, sourceId, folder.id, attempt]);
  return <section className="drive-folder-browser" aria-label="Choose a Drive folder">
    <h4>Connect a folder</h4>
    <p>Choose a folder, then confirm a new read-only Google connection. MDReader lists its direct .md and .markdown files. Existing sources keep their current selection.</p>
    <div className="provider-panel__toolbar">
      {trail.length > 1 ? <button type="button" disabled={busy || loading} onClick={() => setTrail(value => value.slice(0, -1))}>Up one folder</button> : null}
      <span>{folder.name}</span>
      {folder.id !== 'root' ? <button type="button" disabled={busy || loading || !!error} onClick={() => onChoose({ id: folder.id })}>Connect this folder</button> : null}
    </div>
    {loading ? <p role="status">Loading Drive folders…</p> : null}
    {error ? <div><p role="alert">{error}</p><button type="button" disabled={busy || loading} onClick={() => setAttempt(value => value + 1)}>Retry folders</button></div> : null}
    {!loading && !error && page?.folders.length === 0 ? <p>No child folders in this location.</p> : null}
    <ul>{page?.folders.map(item => <li key={item.id}>
      <button type="button" disabled={busy || loading || trail.length >= 32} onClick={() => setTrail(value => [...value, item])}>Browse {item.name}</button>
    </li>)}</ul>
    {page?.nextCursor && page.folders.length < 1000 ? <button type="button" disabled={busy || loading} onClick={() => void load(page.nextCursor, true)}>Load more folders</button> : null}
    {page?.nextCursor && page.folders.length >= 1000 ? <p>Showing the first 1,000 folders in this location.</p> : null}
    {trail.length >= 32 ? <p>This folder browser supports up to 32 nested levels.</p> : null}
  </section>;
}
