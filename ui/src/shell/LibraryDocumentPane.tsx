import { useEffect, useMemo, useState } from 'react';
import { MarkdownReader, PlatformProvider } from '@mdreader/ui';
import { parseMarkdownAuto } from '@mdreader/markdown/worker';
import type { ParsedDoc } from '@mdreader/markdown';

import type {
  EntityRow,
  LibraryEntityQueryResponse_Serialize,
  LibrarySourceExecution,
} from '../bindings.js';
import { createReaderPlatform } from '../platform/reader-platform.js';
import { LibrarySemanticInspector } from './LibrarySemanticInspector.js';
import { physicalRecordKey } from './library-documents.js';

export interface LibraryDocumentPaneProps {
  member: LibrarySourceExecution;
  row: EntityRow;
  evidence: LibraryEntityQueryResponse_Serialize;
  onEdit?: () => void;
}

function fieldsOf(row: EntityRow): Record<string, unknown> {
  if (!row.fields || typeof row.fields !== 'object' || Array.isArray(row.fields)) return {};
  return row.fields as Record<string, unknown>;
}

function originalUrl(member: LibrarySourceExecution, recordKey: string): string | null {
  if (member.kind === 'google-drive') {
    return `https://drive.google.com/open?id=${encodeURIComponent(recordKey)}`;
  }
  if (member.kind === 'dropbox') {
    return `https://www.dropbox.com/home${recordKey
      .split('/')
      .map((part) => encodeURIComponent(part))
      .join('/')}`;
  }
  return null;
}

function saveCopy(name: string, body: string): void {
  const safeName = name.replaceAll(/[/\\:\0]/g, '-').slice(0, 240) || 'document.md';
  const url = URL.createObjectURL(new Blob([body], { type: 'text/markdown;charset=utf-8' }));
  const link = document.createElement('a');
  link.href = url;
  link.download = safeName;
  link.click();
  URL.revokeObjectURL(url);
}

export function LibraryDocumentPane({
  member,
  row,
  evidence,
  onEdit,
}: LibraryDocumentPaneProps) {
  const values = fieldsOf(row);
  const title =
    typeof values.title === 'string' && values.title.length > 0
      ? values.title
      : row.record_key;
  const body =
    typeof values.body === 'string' ? values.body : null;
  const [doc, setDoc] = useState<ParsedDoc | null>(null);
  const [inspectorOpen, setInspectorOpen] = useState(false);
  const platform = useMemo(() => createReaderPlatform(null), []);
  const recordKey = physicalRecordKey(row);
  const providerUrl = recordKey === null ? null : originalUrl(member, recordKey);

  useEffect(() => {
    let current = true;
    setDoc(null);
    setInspectorOpen(false);
    if (body === null) {
      return () => {
        current = false;
      };
    }
    void parseMarkdownAuto(body).then((parsed) => {
      if (current) setDoc(parsed);
    });
    return () => {
      current = false;
    };
  }, [body]);

  return (
    <>
      <div className="reader-region library-reader">
        <div className="reader-toolbar library-reader__toolbar">
          <nav className="breadcrumb" aria-label="Document location">
            <span className="breadcrumb__crumb">{member.name}</span>
            <span className="breadcrumb__sep">/</span>
            <span className="breadcrumb__leaf">{title}</span>
          </nav>
          {member.editable ? (
            <button type="button" onClick={onEdit} disabled={!onEdit}>
              Edit local file
            </button>
          ) : (
            <span className="library-reader__state">Read-only</span>
          )}
          {body !== null ? (
            <button
              type="button"
              className="library-reader__inspect"
              aria-expanded={inspectorOpen}
              onClick={() => setInspectorOpen((open) => !open)}
            >
              {inspectorOpen ? 'Close Inspector' : 'Inspect'}
            </button>
          ) : null}
          {providerUrl ? (
            <a href={providerUrl} target="_blank" rel="noreferrer">
              Open original
            </a>
          ) : null}
          <button
            type="button"
            disabled={body === null}
            onClick={() => body && saveCopy(title, body)}
          >
            Download
          </button>
        </div>
        {!member.editable ? (
          <p className="library-reader__policy">
            Read from {member.name}. Changes remain in the source.
          </p>
        ) : null}
        <div className="reader-body">
          <div className="reader-host">
            {body === null ? (
              <div className="reader-empty">
                <p className="reader-empty__hint">
                  Could not read this document: the provider body is unavailable.
                </p>
              </div>
            ) : doc ? (
              <PlatformProvider platform={platform}>
                <MarkdownReader doc={doc} />
              </PlatformProvider>
            ) : (
              <div className="reader-empty">Reading provider-owned content…</div>
            )}
          </div>
        </div>
      </div>
      {body !== null && inspectorOpen ? (
        <aside className="library-reader__inspector" aria-label="Semantic Inspector">
          <LibrarySemanticInspector
            evidence={evidence}
            row={row}
          />
        </aside>
      ) : null}
    </>
  );
}
