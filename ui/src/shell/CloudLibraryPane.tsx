import { useEffect, useMemo, useState } from 'react';
import { MarkdownReader, PlatformProvider } from '@mdreader/ui';
import { parseMarkdownAuto } from '@mdreader/markdown/worker';
import type { ParsedDoc } from '@mdreader/markdown';

import type {
  CloudDocumentQueryResponse,
  CloudDocumentRow,
  CloudProvider,
  CloudSourceDto,
} from '../bindings.js';
import { createReaderPlatform } from '../platform/reader-platform.js';
import { CloudSemanticInspector } from './CloudSemanticInspector.js';

export interface CloudLibraryPaneProps {
  source: CloudSourceDto;
  row: CloudDocumentRow;
  evidence: CloudDocumentQueryResponse;
}

function fieldsOf(row: CloudDocumentRow): Record<string, unknown> {
  if (!row.fields || typeof row.fields !== 'object' || Array.isArray(row.fields)) return {};
  return row.fields as Record<string, unknown>;
}

function originalUrl(provider: CloudProvider, recordKey: string): string {
  if (provider === 'google-drive') {
    return `https://drive.google.com/open?id=${encodeURIComponent(recordKey)}`;
  }
  return `https://www.dropbox.com/home${recordKey
    .split('/')
    .map((part) => encodeURIComponent(part))
    .join('/')}`;
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

export function CloudLibraryPane({
  source,
  row,
  evidence,
}: CloudLibraryPaneProps) {
  const values = fieldsOf(row);
  const title =
    typeof values.title === 'string' && values.title.length > 0 ? values.title : row.record_key;
  const body =
    typeof values.body === 'string' ? values.body : null;
  const [doc, setDoc] = useState<ParsedDoc | null>(null);
  const platform = useMemo(() => createReaderPlatform(null), []);

  useEffect(() => {
    let current = true;
    setDoc(null);
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
      <div className="reader-region cloud-reader">
        <div className="reader-toolbar cloud-reader__toolbar">
          <nav className="breadcrumb" aria-label="Document location">
            <span className="breadcrumb__crumb">{source.label}</span>
            <span className="breadcrumb__sep">/</span>
            <span className="breadcrumb__leaf">{title}</span>
          </nav>
          <span className="cloud-reader__state">Read-only</span>
          <a href={originalUrl(source.provider, row.record_key)} target="_blank" rel="noreferrer">
            Open original
          </a>
          <button
            type="button"
            disabled={body === null}
            onClick={() => body && saveCopy(title, body)}
          >
            Download
          </button>
        </div>
        <p className="cloud-reader__policy">
          Read through from {source.label}; this document is not copied into the local index.
        </p>
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
      {body !== null ? (
        <aside className="cloud-reader__inspector" aria-label="Cloud Semantic Inspector">
          <CloudSemanticInspector evidence={evidence} row={row} />
        </aside>
      ) : null}
    </>
  );
}
