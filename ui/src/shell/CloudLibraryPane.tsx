import { useCallback, useEffect, useMemo, useState } from 'react';
import { MarkdownReader, PlatformProvider } from '@mdreader/ui';
import { parseMarkdownAuto } from '@mdreader/markdown/worker';
import type { ParsedDoc } from '@mdreader/markdown';

import type {
  CloudDocumentQueryResponse,
  CloudDocumentRow,
  CloudProvider,
  CloudSourceDto,
} from '../bindings.js';
import type { FederatedDocument } from '../platform/semantics.js';
import { createReaderPlatform } from '../platform/reader-platform.js';
import { DocumentInspector } from './lens/DocumentInspector.js';
import { CLOUD_DOCUMENT_FIELDS } from './cloud-selection.js';

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

function normalizeCloudDocument(
  evidence: CloudDocumentQueryResponse,
  row: CloudDocumentRow,
): FederatedDocument {
  const values = fieldsOf(row);
  const fields = row.provenance.map((field) => {
    const value = values[field.field] ?? null;
    const providers = field.providers.map((provider) => ({
      source: provider.source_id,
      attribute: provider.physical_attribute ?? 'physical attribute unavailable',
      verdict:
        provider.observation === 'observed'
          ? 'observed'
          : provider.observation === 'observed-nothing'
            ? 'observed-nothing'
            : 'not-asked',
      value: provider.observation === 'observed' ? value : null,
      reason: [
        `Provider ${provider.provider}`,
        `record ${provider.record_key}`,
        `mapping ${provider.mapping_version}`,
        provider.observed_at_ms === null
          ? 'observed time unavailable'
          : `observed at ${new Date(provider.observed_at_ms).toISOString()}`,
      ].join(' · '),
      declaredFallback: [] as string[],
    }));
    const winner = field.providers.find((provider) => provider.observation === 'observed') ?? null;
    return {
      name: field.field,
      value,
      winner: winner?.source_id ?? null,
      providers,
      explanation: winner
        ? `\`${field.field}\` answered by \`${winner.source_id}.${winner.physical_attribute ?? 'physical attribute unavailable'}\``
        : `No provider observed \`${field.field}\``,
    };
  });
  return {
    queryId: evidence.query_id,
    completeness: evidence.completeness,
    schema: {
      name: 'document',
      version: evidence.version_pins.schema_version,
      checksum: evidence.version_pins.schema_checksum,
      entities: ['Document'],
      arrowCount: 0,
      attributeCount: CLOUD_DOCUMENT_FIELDS.length,
      equationCount: 0,
      approvalRequired: false,
      objectCount: evidence.rows.length,
      proposalCount: 0,
    },
    document: {
      path:
        typeof values.path === 'string' && values.path.length > 0
          ? values.path
          : row.record_key,
      fields,
    },
    sources: evidence.sources.map((report) => ({
      source: report.source_id,
      outcome: report.outcome.kind,
      execution: report.outcome.mode,
      freshness: report.freshness_ms === null ? null : 'live',
      ageMs: report.freshness_ms,
      scanned: report.outcome.scanned,
      returned: report.outcome.returned,
      pushed: report.outcome.pushed,
      residual: report.outcome.residual,
      refusal: null,
    })),
  };
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

  const inspectorEvidence = useMemo(
    () => normalizeCloudDocument(evidence, row),
    [evidence, row],
  );
  const queryInspector = useCallback(async () => inspectorEvidence, [inspectorEvidence]);
  const cancelInspector = useCallback(async () => {}, []);

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
          <DocumentInspector
            path={row.record_key}
            workspaceKey={source.id}
            workspaceGeneration={0}
            query={queryInspector}
            cancel={cancelInspector}
          />
        </aside>
      ) : null}
    </>
  );
}
