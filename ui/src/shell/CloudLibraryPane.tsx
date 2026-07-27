import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { MarkdownReader, PlatformProvider } from '@mdreader/ui';
import { parseMarkdownAuto } from '@mdreader/markdown/worker';
import type { ParsedDoc } from '@mdreader/markdown';

import type {
  CloudDocumentQueryResponse,
  CloudDocumentRow,
  CloudFilterInput,
  CloudProvider,
  CloudSourceDto,
} from '../bindings.js';
import type { FederatedDocument } from '../platform/semantics.js';
import { createReaderPlatform } from '../platform/reader-platform.js';
import { DocumentInspector } from './lens/DocumentInspector.js';

const DOCUMENT_FIELDS = [
  'providerId',
  'title',
  'path',
  'mediaType',
  'modifiedAt',
  'size',
  'revision',
  'checksum',
  'body',
];

export interface CloudLibraryPaneProps {
  source: CloudSourceDto;
  row: CloudDocumentRow;
  query: (
    provider: CloudProvider,
    filters: CloudFilterInput[],
    fields: string[],
    requestId: string,
  ) => Promise<CloudDocumentQueryResponse>;
  cancel: (requestId: string) => Promise<boolean>;
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
  response: CloudDocumentQueryResponse,
  row: CloudDocumentRow,
): FederatedDocument {
  const values = fieldsOf(row);
  const report = response.sources[0];
  const sourceOutcome = report?.outcome;
  const fields = row.provenance.map((field) => {
    const value = values[field.field] ?? null;
    const providers = field.providers.map((provider) => ({
      source: provider.source_id,
      attribute: field.field,
      verdict: provider.observation,
      value: provider.observation === 'observed' ? value : null,
      reason:
        provider.observed_at_ms === null
          ? null
          : `Observed at ${new Date(provider.observed_at_ms).toISOString()}`,
      declaredFallback: [] as string[],
    }));
    const winner = field.providers.find((provider) => provider.observation === 'observed');
    return {
      name: field.field,
      value,
      winner: winner?.source_id ?? null,
      providers,
      explanation: winner
        ? `\`${field.field}\` answered by \`${winner.source_id}.${field.field}\``
        : `No provider observed \`${field.field}\``,
    };
  });
  return {
    queryId: response.query_id,
    completeness: response.completeness,
    schema: {
      name: 'library',
      version: response.version_pins.schema_version,
      checksum: response.version_pins.schema_checksum,
      entities: ['Document'],
      arrowCount: 0,
      attributeCount: DOCUMENT_FIELDS.length,
      equationCount: 0,
      approvalRequired: false,
      objectCount: response.rows.length,
      proposalCount: 0,
    },
    document: {
      path:
        typeof values.path === 'string' && values.path.length > 0
          ? values.path
          : row.record_key,
      fields,
    },
    sources: report && sourceOutcome
      ? [
          {
            source: report.source_id,
            outcome: sourceOutcome.kind,
            execution: sourceOutcome.mode,
            freshness: 'live',
            ageMs: null,
            scanned: sourceOutcome.scanned,
            returned: sourceOutcome.returned,
            pushed: sourceOutcome.pushed,
            residual: sourceOutcome.residual,
            refusal: null,
          },
        ]
      : [],
  };
}

export function CloudLibraryPane({
  source,
  row,
  query,
  cancel,
}: CloudLibraryPaneProps) {
  const queryRef = useRef(query);
  queryRef.current = query;
  const cancelRef = useRef(cancel);
  cancelRef.current = cancel;
  const [resolvedRow, setResolvedRow] = useState<CloudDocumentRow | null>(null);
  const [readError, setReadError] = useState<string | null>(null);
  const values = fieldsOf(resolvedRow ?? row);
  const title =
    typeof values.title === 'string' && values.title.length > 0 ? values.title : row.record_key;
  const body =
    resolvedRow && typeof fieldsOf(resolvedRow).body === 'string'
      ? (fieldsOf(resolvedRow).body as string)
      : null;
  const [doc, setDoc] = useState<ParsedDoc | null>(null);
  const platform = useMemo(() => createReaderPlatform(null), []);

  useEffect(() => {
    let retired = false;
    const bodyRequestId =
      `cloud-reader:${source.provider}:${source.id}:${row.record_key}:` +
      globalThis.crypto.randomUUID();
    setResolvedRow(null);
    setReadError(null);
    void queryRef
      .current(
        source.provider,
        [{ field: 'path', operator: 'eq', value: row.record_key }],
        DOCUMENT_FIELDS,
        bodyRequestId,
      )
      .then((answer) => {
        if (retired) return;
        const selected = answer.rows.find(
          (candidate) =>
            candidate.source_id === source.id && candidate.record_key === row.record_key,
        );
        if (!selected || typeof fieldsOf(selected).body !== 'string') {
          throw new Error('The provider did not return the selected document body.');
        }
        setResolvedRow(selected);
      })
      .catch((cause: unknown) => {
        if (retired) return;
        setReadError(cause instanceof Error ? cause.message : String(cause));
      });
    return () => {
      retired = true;
      void cancelRef.current(bodyRequestId);
    };
  }, [row.record_key, source.id, source.provider]);

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

  const requestId = useCallback(
    (invocation: string, request: string) =>
      `cloud-inspector:${source.provider}:${invocation}:${request}`,
    [source.provider],
  );
  const queryInspector = useCallback(
    async (_path: string, invocation: string, request: string) => {
      const answer = await queryRef.current(
        source.provider,
        [{ field: 'path', operator: 'eq', value: row.record_key }],
        DOCUMENT_FIELDS,
        requestId(invocation, request),
      );
      const selected = answer.rows.find((candidate) => candidate.record_key === row.record_key);
      if (!selected) throw new Error('The provider did not return the selected document.');
      return normalizeCloudDocument(answer, selected);
    },
    [requestId, row.record_key, source.provider],
  );
  const cancelInspector = useCallback(
    async (invocation: string, request: string) => {
      await cancelRef.current(requestId(invocation, request));
    },
    [requestId],
  );

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
            {readError ? (
              <div className="reader-empty">
                <p className="reader-empty__hint">Could not read this document: {readError}</p>
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
      <aside className="cloud-reader__inspector" aria-label="Cloud Semantic Inspector">
        <DocumentInspector
          path={row.record_key}
          workspaceKey={source.id}
          workspaceGeneration={0}
          query={queryInspector}
          cancel={cancelInspector}
        />
      </aside>
    </>
  );
}
