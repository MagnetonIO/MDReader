import { useEffect, useRef, useState } from 'react';

import type {
  FederatedDocument,
  FederatedField,
  FederatedProvider,
} from '../../platform/semantics.js';

type InspectorTab = 'result' | 'sources' | 'provenance' | 'schema';

export interface DocumentInspectorProps {
  path: string;
  workspaceKey: string | null;
  query: (path: string) => Promise<FederatedDocument>;
}

const TABS: Array<{ id: InspectorTab; label: string }> = [
  { id: 'result', label: 'Result' },
  { id: 'sources', label: 'Sources' },
  { id: 'provenance', label: 'Provenance' },
  { id: 'schema', label: 'Schema' },
];

function valueText(value: string | null): string {
  if (value === null) return 'Not provided';
  try {
    const parsed: unknown = JSON.parse(value);
    if (typeof parsed === 'string') return parsed;
    return JSON.stringify(parsed);
  } catch {
    return value;
  }
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

export function DocumentInspector({ path, workspaceKey, query }: DocumentInspectorProps) {
  const [tab, setTab] = useState<InspectorTab>('result');
  const [answer, setAnswer] = useState<FederatedDocument | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(true);
  const requestRef = useRef(0);

  useEffect(() => {
    const request = (requestRef.current += 1);
    setAnswer(null);
    setError(null);
    setLoading(true);
    void query(path)
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
    };
  }, [path, query, workspaceKey]);

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
            aria-selected={tab === item.id}
            className={`document-inspector__tab${tab === item.id ? ' is-active' : ''}`}
            onClick={() => setTab(item.id)}
          >
            {item.label}
          </button>
        ))}
      </div>

      {loading ? <p className="lens-rail__empty">Reading the document across sources…</p> : null}
      {error ? <p className="lens-rail__empty">Could not assemble this document — {error}</p> : null}
      {answer && !loading ? (
        <div role="tabpanel" className="document-inspector__panel">
          {tab === 'result' ? <ResultTab answer={answer} /> : null}
          {tab === 'sources' ? <SourcesTab answer={answer} /> : null}
          {tab === 'provenance' ? <ProvenanceTab answer={answer} /> : null}
          {tab === 'schema' ? <SchemaTab answer={answer} /> : null}
        </div>
      ) : null}
    </section>
  );
}

function ResultTab({ answer }: { answer: FederatedDocument }) {
  return (
    <>
      <p className="document-inspector__path lens-mono">{answer.document.path}</p>
      <dl className="document-result">
        {answer.document.fields.map((field) => (
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
      {answer.document.fields.map((field) => (
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
  const version =
    answer.schema.version === null
      ? `${answer.schema.name} · not catalogued yet`
      : `${answer.schema.name} · v${answer.schema.version}`;
  return (
    <div className="document-schema">
      <h3>{version}</h3>
      <p className="lens-mono">{answer.schema.checksum ?? 'No persisted checksum yet'}</p>
      <p>
        This semantic view assembles one Document from filesystem, Git, and frontmatter evidence.
        Changing the semantic view changes the semantic interpretation, not the Markdown file or
        its format.
      </p>
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
