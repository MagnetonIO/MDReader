import { useEffect, useMemo, useState } from 'react';
import { MarkdownReader, PlatformProvider } from '@mdreader/ui';
import { parseMarkdownAuto } from '@mdreader/markdown/worker';
import type { ParsedDoc } from '@mdreader/markdown';

import { readDoc } from '../platform/fs';
import { createReaderPlatform } from '../platform/reader-platform';
import { makeSemanticsAdapter } from '../platform/semantics';
import type { TabState } from '../stores/types';
import { DocumentInspector } from '../shell/lens/DocumentInspector';
import {
  readOfflineDocument,
  writeOfflineDocument,
  type MobileSourceIdentity,
  type OfflineCacheRead,
} from './offline-cache';

export interface MobileReaderProps {
  tab: TabState;
  mode: 'reader' | 'inspector';
  sourceIdentity?: MobileSourceIdentity | null;
  onBack: () => void;
  onModeChange: (mode: 'reader' | 'inspector') => void;
}

function dirname(path: string): string {
  const normalized = path.replace(/[\\/]+$/, '');
  const index = Math.max(normalized.lastIndexOf('/'), normalized.lastIndexOf('\\'));
  return index >= 0 ? normalized.slice(0, index) : normalized;
}

/** A read-only mobile document surface. It never imports or mounts the editor. */
export function MobileReader({
  tab,
  mode,
  sourceIdentity = null,
  onBack,
  onModeChange,
}: MobileReaderProps) {
  const [doc, setDoc] = useState<ParsedDoc | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [offline, setOffline] = useState<OfflineCacheRead | null>(null);
  const semantics = useMemo(makeSemanticsAdapter, []);
  const readerPlatform = useMemo(
    () => createReaderPlatform(dirname(tab.docRef.path)),
    [tab.docRef.path],
  );

  useEffect(() => {
    let current = true;
    setDoc(null);
    setError(null);
    setOffline(null);
    void readDoc(tab.docRef.path)
      .then(async (payload) => {
        const parsed = await parseMarkdownAuto(payload.content);
        if (sourceIdentity && tab.originRoot) {
          const cachedAt = Date.now();
          void writeOfflineDocument({
            source: sourceIdentity,
            sourceRoot: tab.originRoot,
            documentPath: tab.docRef.path,
            revision: payload.hash,
            observedAt: payload.mtime,
            cachedAt,
            freshUntil: cachedAt + 5 * 60_000,
            expiresAt: cachedAt + 7 * 24 * 60 * 60_000,
            content: payload.content,
            bytes: new TextEncoder().encode(payload.content).byteLength,
          });
        }
        return parsed;
      })
      .then((parsed) => {
        if (current) setDoc(parsed);
      })
      .catch(async () => {
        const cached = await readOfflineDocument(tab.originRoot, tab.docRef.path);
        if (!current) return;
        setOffline(cached);
        if (cached.status === 'hit') {
          try {
            setDoc(await parseMarkdownAuto(cached.entry.content));
          } catch {
            setOffline({ status: 'corrupt' });
          }
        } else {
          setError(
            cached.status === 'expired'
              ? 'Offline copy expired'
              : cached.status === 'corrupt'
                ? 'Offline copy is corrupt'
                : cached.status === 'unavailable'
                  ? 'Offline cache unavailable'
                  : 'No offline copy',
          );
        }
      });
    return () => {
      current = false;
    };
  }, [sourceIdentity, tab.docRef.path, tab.originRoot]);

  return (
    <main className="mobile-reader" aria-label={mode === 'reader' ? 'Reader' : 'Semantic Inspector'}>
      <header className="mobile-reader__bar">
        <button type="button" className="mobile-touch" onClick={onBack}>
          Back
        </button>
        <strong>{tab.docRef.name}</strong>
        <button
          type="button"
          className="mobile-touch"
          onClick={() => onModeChange(mode === 'reader' ? 'inspector' : 'reader')}
        >
          {mode === 'reader' ? 'Evidence' : 'Document'}
        </button>
      </header>
      {mode === 'inspector' ? (
        <div className="mobile-reader__inspector">
          <DocumentInspector
            path={tab.docRef.path}
            workspaceKey={tab.originRoot}
            workspaceGeneration={0}
            query={semantics.queryFederatedDocument}
            cancel={semantics.cancelFederatedDocument}
          />
        </div>
      ) : error ? (
        <div className="mobile-state" role="status">
          <h2>{error}</h2>
          {offline?.status === 'expired' ? (
            <p>
              Source {offline.entry.source.id} · revision {offline.entry.revision}
            </p>
          ) : null}
        </div>
      ) : doc ? (
        <>
          {offline?.status === 'hit' ? (
            <aside className="mobile-offline-notice" role="status">
              Offline copy · {offline.freshness} · source {offline.entry.source.id} · revision{' '}
              {offline.entry.revision}. This is one cached document, not a complete library.
            </aside>
          ) : null}
          <div className="mobile-reader__document">
            <PlatformProvider platform={readerPlatform}>
              <MarkdownReader doc={doc} />
            </PlatformProvider>
          </div>
        </>
      ) : (
        <div className="mobile-state" role="status">
          Loading document…
        </div>
      )}
    </main>
  );
}
