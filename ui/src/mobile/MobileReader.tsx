import { useEffect, useMemo, useRef, useState } from 'react';
import { MarkdownReader, PlatformProvider } from '@mdreader/ui';
import { parseMarkdownAuto } from '@mdreader/markdown/worker';
import type { ParsedDoc } from '@mdreader/markdown';

import { readDoc } from '../platform/fs';
import { createReaderPlatform } from '../platform/reader-platform';
import type { TabState } from '../stores/types';
import {
  MobileSourceReadError,
  type MobileDocumentPayload,
  type MobileDocumentProvenance,
} from './mobile-source-adapter';
import {
  readOfflineDocument,
  writeOfflineDocument,
  type MobileDocumentIdentity,
  type OfflineCacheRead,
} from './offline-cache';

export interface MobileReaderProps {
  tab: TabState;
  mode: 'reader' | 'inspector';
  sourceIdentity: MobileDocumentIdentity;
  loadDocument?: () => Promise<MobileDocumentPayload>;
  onBack: () => void;
  onModeChange: (mode: 'reader' | 'inspector') => void;
}

function dirname(path: string): string {
  const normalized = path.replace(/[\\/]+$/, '');
  const index = Math.max(normalized.lastIndexOf('/'), normalized.lastIndexOf('\\'));
  return index >= 0 ? normalized.slice(0, index) : normalized;
}

function sameIdentity(
  left: MobileDocumentIdentity,
  right: MobileDocumentIdentity,
): boolean {
  return (
    left.source.kind === right.source.kind &&
    left.source.id === right.source.id &&
    left.sourceRoot.id === right.sourceRoot.id &&
    left.sourceRoot.version === right.sourceRoot.version &&
    left.document.id === right.document.id &&
    left.document.path === right.document.path
  );
}

function observationStateLabel(state: { state: string; value?: unknown }): string {
  if (state.state === 'observed_value') return 'observed value';
  if (state.state === 'observed_null') return 'observed null';
  if (state.state === 'field_unseen') return 'field unseen';
  if (state.state === 'record_unseen') return 'record unseen';
  if (state.state === 'not_asked') {
    const value =
      typeof state.value === 'object' && state.value !== null
        ? (state.value as Record<string, unknown>)
        : null;
    return `not asked${typeof value?.reason === 'string' ? `: ${value.reason}` : ''}`;
  }
  return 'unknown';
}

/** A read-only mobile document surface. It never imports or mounts the editor. */
export function MobileReader({
  tab,
  mode,
  sourceIdentity,
  loadDocument,
  onBack,
  onModeChange,
}: MobileReaderProps) {
  const [doc, setDoc] = useState<ParsedDoc | null>(null);
  const [error, setError] = useState<{ kind: string; message: string } | null>(null);
  const [offline, setOffline] = useState<OfflineCacheRead | null>(null);
  const [evidence, setEvidence] = useState<{
    revision: string;
    observedAt: number;
    provenance: MobileDocumentProvenance | null;
  } | null>(null);
  const requestEpoch = useRef(0);
  const readerPlatform = useMemo(
    () => createReaderPlatform(dirname(tab.docRef.path)),
    [tab.docRef.path],
  );

  useEffect(() => {
    const epoch = ++requestEpoch.current;
    const current = () => requestEpoch.current === epoch;
    setDoc(null);
    setError(null);
    setOffline(null);
    setEvidence(null);
    const liveRead = loadDocument
      ? loadDocument()
      : readDoc(tab.docRef.path).then((payload) => ({
          ...sourceIdentity,
          content: payload.content,
          revision: payload.hash,
          observedAt: payload.mtime,
          provenance: null,
        }));
    void liveRead
      .then(async (payload) => {
        if (!sameIdentity(sourceIdentity, payload)) {
          throw new MobileSourceReadError(
            'identity-mismatch',
            'The mobile document identity changed during the read.',
          );
        }
        const parsed = await parseMarkdownAuto(payload.content);
        if (!current()) return null;
        const cachedAt = Date.now();
        await writeOfflineDocument({
          ...sourceIdentity,
          revision: payload.revision,
          observedAt: payload.observedAt,
          cachedAt,
          freshUntil: cachedAt + 5 * 60_000,
          expiresAt: cachedAt + 7 * 24 * 60 * 60_000,
          content: payload.content,
          bytes: new TextEncoder().encode(payload.content).byteLength,
          provenance: payload.provenance,
        });
        if (!current()) return null;
        return { parsed, payload };
      })
      .then((result) => {
        if (current() && result) {
          setDoc(result.parsed);
          setEvidence({
            revision: result.payload.revision,
            observedAt: result.payload.observedAt,
            provenance: result.payload.provenance,
          });
        }
      })
      .catch(async (cause: unknown) => {
        const cached = await readOfflineDocument(sourceIdentity);
        if (!current()) return;
        setOffline(cached);
        if (cached.status === 'hit') {
          try {
            const parsed = await parseMarkdownAuto(cached.entry.content);
            if (current()) {
              setDoc(parsed);
              setEvidence({
                revision: cached.entry.revision,
                observedAt: cached.entry.observedAt,
                provenance: cached.entry.provenance,
              });
            }
          } catch {
            if (current()) {
              setOffline({ status: 'corrupt' });
              setError({ kind: 'offline-corrupt', message: 'Offline copy is corrupt' });
            }
          }
        } else {
          const message =
            cached.status === 'expired'
              ? 'Offline copy expired'
              : cached.status === 'corrupt'
                ? 'Offline copy is corrupt'
                : cached.status === 'unavailable'
                  ? 'Offline cache unavailable'
                  : 'No offline copy';
          const sourceMessage =
            cause instanceof Error && cause.name === 'MobileSourceReadError'
              ? `${cause.message} `
              : '';
          setError({ kind: `offline-${cached.status}`, message: `${sourceMessage}${message}` });
        }
      });
    return () => {
      if (requestEpoch.current === epoch) requestEpoch.current += 1;
    };
  }, [
    sourceIdentity.document.id,
    sourceIdentity.document.path,
    sourceIdentity.source.id,
    sourceIdentity.source.kind,
    sourceIdentity.sourceRoot.id,
    sourceIdentity.sourceRoot.version,
    loadDocument,
    tab.docRef.path,
    tab.originRoot,
  ]);

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
          <h2>Semantic evidence</h2>
          {error ? (
            <p role="status">{error.message}</p>
          ) : evidence ? (
            <dl className="mobile-evidence">
              <div>
                <dt>Source</dt>
                <dd>{sourceIdentity.source.id}</dd>
              </div>
              <div>
                <dt>Source root</dt>
                <dd>{sourceIdentity.sourceRoot.id}</dd>
              </div>
              <div>
                <dt>Root version</dt>
                <dd>{sourceIdentity.sourceRoot.version}</dd>
              </div>
              <div>
                <dt>Document</dt>
                <dd>{sourceIdentity.document.id}</dd>
              </div>
              <div>
                <dt>Revision</dt>
                <dd>{evidence.revision}</dd>
              </div>
              {evidence.provenance ? (
                <>
                  <div>
                    <dt>Catalog resolver</dt>
                    <dd>{evidence.provenance.pins.catalog_resolver_id}</dd>
                  </div>
                  <div>
                    <dt>Schema version</dt>
                    <dd>{evidence.provenance.pins.schema_version_id}</dd>
                  </div>
                  <div>
                    <dt>Physical schema versions</dt>
                    <dd>
                      {evidence.provenance.pins.physical_schema_version_ids.join(', ') ||
                        'None'}
                    </dd>
                  </div>
                  <div>
                    <dt>Mapping versions</dt>
                    <dd>{evidence.provenance.pins.mapping_version_ids.join(', ')}</dd>
                  </div>
                  <div>
                    <dt>Policy versions</dt>
                    <dd>{evidence.provenance.pins.policy_version_ids.join(', ')}</dd>
                  </div>
                  <div>
                    <dt>Considered providers</dt>
                    <dd>
                      {evidence.provenance.sources
                        .map(
                          (source) =>
                            `${source.source_id} (${source.source_version_id}; ${source.outcome})`,
                        )
                        .join(', ')}
                    </dd>
                  </div>
                  {Object.entries(evidence.provenance.fields).map(
                    ([field, fieldProvenance]) => (
                      <div key={field}>
                        <dt>Field {field}</dt>
                        <dd>
                          {fieldProvenance.considered.map((observation, index) => (
                            <span key={`${observation.source_version_id}:${index}`}>
                              {index === fieldProvenance.selected ? 'Selected' : 'Considered'} ·{' '}
                              {observation.source_version_id} · {observation.record_key} ·{' '}
                              {observation.mapping_version_id} ·{' '}
                              {observation.observed_at_ms ?? 'no observation time'} ·{' '}
                              {observationStateLabel(observation.state)}
                              {index + 1 < fieldProvenance.considered.length ? '; ' : ''}
                            </span>
                          ))}
                        </dd>
                      </div>
                    ),
                  )}
                </>
              ) : (
                <div>
                  <dt>Availability</dt>
                  <dd>
                    Detailed semantic provenance is unavailable; the admitted source and
                    revision remain visible.
                  </dd>
                </div>
              )}
            </dl>
          ) : (
            <p role="status">Loading semantic evidence…</p>
          )}
        </div>
      ) : error ? (
        <div className="mobile-state" role="status">
          <h2>{error.message}</h2>
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
