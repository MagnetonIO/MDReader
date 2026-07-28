import { useEffect, useId, useRef, useState } from 'react';

import type {
  FederatedDocument,
  FederatedField,
  FederatedProvider,
} from '../../platform/semantics.js';

type InspectorTab = 'result' | 'sources' | 'provenance' | 'schema';

export interface DocumentInspectorProps {
  path: string;
  workspaceKey: string | null;
  workspaceGeneration: number;
  query: (path: string, invocation: string, request: string) => Promise<FederatedDocument>;
  cancel: (invocation: string, request: string) => Promise<void>;
}

const TABS: Array<{ id: InspectorTab; label: string }> = [
  { id: 'result', label: 'Result' },
  { id: 'sources', label: 'Sources' },
  { id: 'provenance', label: 'Provenance' },
  { id: 'schema', label: 'Schema' },
];

const DOCUMENT_FIELD_ORDER = ['title', 'author', 'modifiedAt', 'size', 'path', 'body'] as const;

function orderedDocumentFields(fields: FederatedDocument['document']['fields']) {
  return fields
    .map((field, index) => ({
      field,
      index,
      rank: DOCUMENT_FIELD_ORDER.indexOf(field.name as (typeof DOCUMENT_FIELD_ORDER)[number]),
    }))
    .sort((left, right) => {
      const leftRank = left.rank === -1 ? DOCUMENT_FIELD_ORDER.length : left.rank;
      const rightRank = right.rank === -1 ? DOCUMENT_FIELD_ORDER.length : right.rank;
      return leftRank - rightRank || left.index - right.index;
    })
    .map(({ field }) => field);
}

function valueText(value: unknown | null): string {
  if (value === null) return 'Not provided';
  if (typeof value === 'string') return value;
  return JSON.stringify(value) ?? String(value);
}

function providerLabel(provider: FederatedProvider): string {
  return `${provider.source}.${provider.attribute}`;
}

function providerStanding(field: FederatedField, provider: FederatedProvider): string {
  if (provider.verdict === 'observed' && provider.source === field.winner) return 'selected';
  if (provider.verdict === 'observed') return 'also observed';
  if (provider.verdict === 'observed-nothing') return 'no value';
  if (provider.verdict === 'did-not-see-document') return 'did not see document';
  return 'not consulted';
}

export function DocumentInspector({
  path,
  workspaceKey,
  workspaceGeneration,
  query,
  cancel,
}: DocumentInspectorProps) {
  const [tab, setTab] = useState<InspectorTab>('result');
  const [answer, setAnswer] = useState<FederatedDocument | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const requestRef = useRef(0);
  const tabsetId = useId();
  const invocation = useRef(`document-inspector-${tabsetId}`).current;
  const tabRefs = useRef(new Map<InspectorTab, HTMLButtonElement>());

  useEffect(() => {
    const request = (requestRef.current += 1);
    const requestToken = String(request);
    setAnswer(null);
    setError(null);
    setLoading(true);
    void query(path, invocation, requestToken)
      .then((value) => {
        if (requestRef.current !== request) return;
        setAnswer(value);
        setLoading(false);
      })
      .catch((cause: unknown) => {
        if (requestRef.current !== request) return;
        setError(cause instanceof Error ? cause.message : String(cause));
        setLoading(false);
      });
    return () => {
      requestRef.current += 1;
      void cancel(invocation, requestToken);
    };
  }, [cancel, invocation, path, query, workspaceGeneration, workspaceKey]);

  const selectFromKeyboard = (current: InspectorTab, key: string) => {
    const index = TABS.findIndex((item) => item.id === current);
    let next = index;
    if (key === 'ArrowRight') next = (index + 1) % TABS.length;
    else if (key === 'ArrowLeft') next = (index - 1 + TABS.length) % TABS.length;
    else if (key === 'Home') next = 0;
    else if (key === 'End') next = TABS.length - 1;
    else return;
    const nextTab = TABS[next]!.id;
    setTab(nextTab);
    tabRefs.current.get(nextTab)?.focus();
  };

  return (
    <section className="document-inspector" aria-label="Federated document">
      <div className="document-inspector__intro">
        <span className="document-inspector__eyebrow">Federated document</span>
        {answer ? (
          <span
            className={`document-inspector__completeness document-inspector__completeness--${answer.completeness}`}
          >
            {answer.completeness === 'complete' ? 'Complete' : 'Partial'}
          </span>
        ) : null}
      </div>
      <div className="document-inspector__tabs" role="tablist" aria-label="Document evidence">
        {TABS.map((item) => (
          <button
            key={item.id}
            type="button"
            role="tab"
            id={`${tabsetId}-${item.id}-tab`}
            aria-controls={`${tabsetId}-${item.id}-panel`}
            aria-selected={tab === item.id}
            tabIndex={tab === item.id ? 0 : -1}
            className={`document-inspector__tab${tab === item.id ? ' is-active' : ''}`}
            onClick={() => setTab(item.id)}
            onKeyDown={(event) => {
              if (['ArrowRight', 'ArrowLeft', 'Home', 'End'].includes(event.key)) {
                event.preventDefault();
                selectFromKeyboard(item.id, event.key);
              }
            }}
            ref={(element) => {
              if (element) tabRefs.current.set(item.id, element);
              else tabRefs.current.delete(item.id);
            }}
          >
            {item.label}
          </button>
        ))}
      </div>

      {TABS.map((item) => (
        <div
          key={item.id}
          role="tabpanel"
          id={`${tabsetId}-${item.id}-panel`}
          aria-labelledby={`${tabsetId}-${item.id}-tab`}
          className="document-inspector__panel"
          hidden={tab !== item.id}
        >
          {tab === item.id && loading ? (
            <p className="lens-rail__empty">Reading the document across sources…</p>
          ) : null}
          {tab === item.id && error ? (
            <p className="lens-rail__empty">Could not assemble this document — {error}</p>
          ) : null}
          {tab === item.id && answer && !loading && item.id === 'result' ? (
            <ResultTab answer={answer} />
          ) : null}
          {tab === item.id && answer && !loading && item.id === 'sources' ? (
            <SourcesTab answer={answer} />
          ) : null}
          {tab === item.id && answer && !loading && item.id === 'provenance' ? (
            <ProvenanceTab answer={answer} />
          ) : null}
          {tab === item.id && answer && !loading && item.id === 'schema' ? (
            <SchemaTab answer={answer} />
          ) : null}
        </div>
      ))}
    </section>
  );
}

function ResultTab({ answer }: { answer: FederatedDocument }) {
  return (
    <>
      <p className="document-inspector__path lens-mono">{answer.document.path}</p>
      <dl className="document-result">
        {orderedDocumentFields(answer.document.fields).map((field) => (
          <div key={field.name}>
            <dt>{field.name}</dt>
            <dd>
              <span className={field.value === null ? 'is-absent' : undefined}>
                {valueText(field.value)}
              </span>
              {field.winner ? <small>from {field.winner}</small> : <small>no source answered</small>}
            </dd>
          </div>
        ))}
      </dl>
    </>
  );
}

function SourcesTab({ answer }: { answer: FederatedDocument }) {
  return (
    <ol className="document-sources">
      {answer.sources.map((source) => (
        <li key={source.source}>
          <div className="document-sources__head">
            <strong>{source.source}</strong>
            <span>{source.outcome}</span>
          </div>
          {source.outcome === 'ran' ? (
            <>
              <p>
                {source.execution} · {source.freshness}
                {source.freshness === 'cached' ? ` · ${source.ageMs ?? 0} ms old` : ''}
              </p>
              <p>
                {source.scanned} scanned · {source.returned} returned
              </p>
              <p className="lens-mono">
                pushed {source.pushed.join(', ') || 'nothing'} · residual{' '}
                {source.residual.join(', ') || 'nothing'}
              </p>
            </>
          ) : (
            <p>{source.refusal ?? 'The source refused this query.'}</p>
          )}
        </li>
      ))}
    </ol>
  );
}

function ProvenanceTab({ answer }: { answer: FederatedDocument }) {
  return (
    <ol className="document-provenance">
      {orderedDocumentFields(answer.document.fields).map((field) => (
        <li key={field.name}>
          <div className="document-provenance__field">
            <strong>{field.name}</strong>
            <span>{valueText(field.value)}</span>
          </div>
          <ol>
            {field.providers.map((provider) => (
              <li key={`${provider.source}.${provider.attribute}`}>
                <span className="lens-mono">{providerLabel(provider)}</span>
                <span className={`provider-standing provider-standing--${provider.verdict}`}>
                  {providerStanding(field, provider)}
                </span>
                {provider.value !== null ? <small>{valueText(provider.value)}</small> : null}
                {provider.reason ? <small>{provider.reason}</small> : null}
                {provider.declaredFallback.length > 0 ? (
                  <small>tries {provider.declaredFallback.join(' → ')}</small>
                ) : null}
              </li>
            ))}
          </ol>
          <p>{field.explanation.replaceAll('`', '')}</p>
        </li>
      ))}
    </ol>
  );
}

function SchemaTab({ answer }: { answer: FederatedDocument }) {
  const version = `${answer.schema.name} · v${answer.schema.version}`;
  const localSources = new Set(['filesystem', 'git', 'frontmatter']);
  const isLocalMarkdown =
    answer.sources.length > 0 && answer.sources.every((source) => localSources.has(source.source));
  return (
    <div className="document-schema">
      <h3>{version}</h3>
      <p className="lens-mono">{answer.schema.checksum}</p>
      {isLocalMarkdown ? (
        <p>
          This semantic view assembles one Document from filesystem, Git, and frontmatter evidence.
          Changing the semantic view changes the semantic interpretation, not the Markdown file or
          its format.
        </p>
      ) : (
        <p>
          This semantic view assembles one Document from read-only external source evidence.
          Changing the semantic view changes the semantic interpretation, not the provider-owned
          document or its format.
        </p>
      )}
      <dl>
        <div>
          <dt>Entity</dt>
          <dd>{answer.schema.entities.join(', ')}</dd>
        </div>
        <div>
          <dt>Attributes</dt>
          <dd>{answer.schema.attributeCount}</dd>
        </div>
        <div>
          <dt>Laws</dt>
          <dd>{answer.schema.equationCount}</dd>
        </div>
      </dl>
    </div>
  );
}
