import { useEffect, useMemo, useState } from 'react';
import { MarkdownReader, PlatformProvider } from '@mdreader/ui';
import { parseMarkdownAuto } from '@mdreader/markdown/worker';
import type { ParsedDoc } from '@mdreader/markdown';

import { readDoc } from '../platform/fs';
import { createReaderPlatform } from '../platform/reader-platform';
import { makeSemanticsAdapter } from '../platform/semantics';
import type { TabState } from '../stores/types';
import { DocumentInspector } from '../shell/lens/DocumentInspector';

export interface MobileReaderProps {
  tab: TabState;
  mode: 'reader' | 'inspector';
  onBack: () => void;
  onModeChange: (mode: 'reader' | 'inspector') => void;
}

function dirname(path: string): string {
  const normalized = path.replace(/[\\/]+$/, '');
  const index = Math.max(normalized.lastIndexOf('/'), normalized.lastIndexOf('\\'));
  return index >= 0 ? normalized.slice(0, index) : normalized;
}

/** A read-only mobile document surface. It never imports or mounts the editor. */
export function MobileReader({ tab, mode, onBack, onModeChange }: MobileReaderProps) {
  const [doc, setDoc] = useState<ParsedDoc | null>(null);
  const [error, setError] = useState<string | null>(null);
  const semantics = useMemo(makeSemanticsAdapter, []);
  const readerPlatform = useMemo(
    () => createReaderPlatform(dirname(tab.docRef.path)),
    [tab.docRef.path],
  );

  useEffect(() => {
    let current = true;
    setDoc(null);
    setError(null);
    void readDoc(tab.docRef.path)
      .then((payload) => parseMarkdownAuto(payload.content))
      .then((parsed) => {
        if (current) setDoc(parsed);
      })
      .catch((cause: unknown) => {
        if (current) setError(cause instanceof Error ? cause.message : String(cause));
      });
    return () => {
      current = false;
    };
  }, [tab.docRef.path]);

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
          <h2>Document unavailable</h2>
          <p>{error}</p>
        </div>
      ) : doc ? (
        <div className="mobile-reader__document">
          <PlatformProvider platform={readerPlatform}>
            <MarkdownReader doc={doc} />
          </PlatformProvider>
        </div>
      ) : (
        <div className="mobile-state" role="status">
          Loading document…
        </div>
      )}
    </main>
  );
}
