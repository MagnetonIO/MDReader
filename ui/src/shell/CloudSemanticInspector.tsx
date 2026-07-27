import { useId, useRef, useState } from 'react';

import type {
  CloudConsideredProvider,
  CloudDocumentQueryResponse,
  CloudDocumentRow,
  CloudFieldProvenance,
  CloudObservation,
} from '../bindings.js';

type InspectorTab = 'result' | 'sources' | 'provenance' | 'versions';

const TABS: Array<{ id: InspectorTab; label: string }> = [
  { id: 'result', label: 'Result' },
  { id: 'sources', label: 'Sources' },
  { id: 'provenance', label: 'Provenance' },
  { id: 'versions', label: 'Versions' },
];

export interface CloudSemanticInspectorProps {
  evidence: CloudDocumentQueryResponse;
  row: CloudDocumentRow;
}

function valueText(value: unknown): string {
  if (value === null) return 'null';
  if (typeof value === 'string') return value;
  return JSON.stringify(value) ?? String(value);
}

function selectedProvider(
  field: CloudFieldProvenance,
): CloudConsideredProvider | null | undefined {
  if (field.selected === null) return null;
  return field.providers[field.selected];
}

function resultValue(field: CloudFieldProvenance): {
  text: string;
  absent: boolean;
  provider: CloudConsideredProvider | null | undefined;
} {
  const provider = selectedProvider(field);
  if (provider === null) {
    return {
      text: field.providers.map((candidate) => candidate.observation.kind).join(', '),
      absent: true,
      provider,
    };
  }
  if (provider === undefined) {
    return { text: `Invalid selected provider ${field.selected}`, absent: true, provider };
  }
  switch (provider.observation.kind) {
    case 'observed-value':
      return { text: valueText(provider.observation.value), absent: false, provider };
    case 'observed-null':
      return { text: 'null', absent: false, provider };
    case 'observed-nothing':
    case 'unobservable':
      return {
        text: `Selected provider reported ${provider.observation.kind}`,
        absent: true,
        provider,
      };
  }
}

function providerLabel(provider: CloudConsideredProvider): string {
  return provider.physical_attribute === null
    ? provider.source_id
    : `${provider.source_id}.${provider.physical_attribute}`;
}

function observationStanding(
  observation: CloudObservation,
  selected: boolean,
): string {
  const standing = (() => {
    switch (observation.kind) {
      case 'observed-value':
        return 'observed value';
      case 'observed-null':
        return 'observed null';
      case 'observed-nothing':
        return 'observed nothing';
      case 'unobservable':
        return 'unobservable';
    }
  })();
  return selected ? `${standing} · selected` : standing;
}

function observationReason(observation: CloudObservation): string | null {
  switch (observation.kind) {
    case 'observed-value':
    case 'observed-null':
      return null;
    case 'observed-nothing':
    case 'unobservable':
      return observation.reason;
  }
}

export function CloudSemanticInspector({
  evidence,
  row,
}: CloudSemanticInspectorProps) {
  const [tab, setTab] = useState<InspectorTab>('result');
  const tabsetId = useId();
  const tabRefs = useRef(new Map<InspectorTab, HTMLButtonElement>());

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
    <section className="document-inspector" aria-label="Cloud document evidence">
      <div className="document-inspector__intro">
        <span className="document-inspector__eyebrow">Cloud document</span>
        <span
          className={`document-inspector__completeness document-inspector__completeness--${evidence.completeness}`}
        >
          {evidence.completeness === 'complete' ? 'Complete' : evidence.completeness}
        </span>
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
          {tab === item.id && item.id === 'result' ? <ResultTab row={row} /> : null}
          {tab === item.id && item.id === 'sources' ? (
            <SourcesTab evidence={evidence} />
          ) : null}
          {tab === item.id && item.id === 'provenance' ? (
            <ProvenanceTab row={row} />
          ) : null}
          {tab === item.id && item.id === 'versions' ? (
            <VersionsTab evidence={evidence} />
          ) : null}
        </div>
      ))}
    </section>
  );
}

function ResultTab({ row }: { row: CloudDocumentRow }) {
  return (
    <>
      <p className="document-inspector__path lens-mono">{row.record_key}</p>
      <dl className="document-result">
        {row.provenance.map((field) => {
          const result = resultValue(field);
          return (
            <div key={field.field}>
              <dt>{field.field}</dt>
              <dd>
                <span className={result.absent ? 'is-absent' : undefined}>{result.text}</span>
                {result.provider ? (
                  <small>from {providerLabel(result.provider)}</small>
                ) : (
                  <small>No selected provider</small>
                )}
              </dd>
            </div>
          );
        })}
      </dl>
    </>
  );
}

function SourcesTab({ evidence }: { evidence: CloudDocumentQueryResponse }) {
  return (
    <ol className="document-sources">
      {evidence.sources.map((source) => (
        <li key={source.source_id}>
          <div className="document-sources__head">
            <strong>{source.source_id}</strong>
            <span>{source.outcome.kind}</span>
          </div>
          <p>
            {source.outcome.mode} · {source.freshness_ms} ms freshness
          </p>
          <p>
            {source.outcome.scanned} scanned · {source.outcome.returned} returned
          </p>
          <p className="lens-mono">
            pushed {source.outcome.pushed.join(', ') || 'nothing'} · residual{' '}
            {source.outcome.residual.join(', ') || 'nothing'}
          </p>
        </li>
      ))}
    </ol>
  );
}

function ProvenanceTab({ row }: { row: CloudDocumentRow }) {
  return (
    <ol className="document-provenance">
      {row.provenance.map((field) => (
        <li key={field.field}>
          <div className="document-provenance__field">
            <strong>{field.field}</strong>
            <span>
              {field.selected === null ? 'No selected provider' : `selected provider ${field.selected}`}
            </span>
          </div>
          <ol>
            {field.providers.map((provider, index) => {
              const reason = observationReason(provider.observation);
              return (
                <li key={`${provider.source_id}:${provider.record_key}:${index}`}>
                  <span className="lens-mono">{providerLabel(provider)}</span>
                  <span className={`provider-standing provider-standing--${provider.observation.kind}`}>
                    {observationStanding(provider.observation, field.selected === index)}
                  </span>
                  {provider.observation.kind === 'observed-value' ? (
                    <small>{valueText(provider.observation.value)}</small>
                  ) : null}
                  {reason ? <small>{reason}</small> : null}
                  <small>
                    provider {provider.provider} · record {provider.record_key} · mapping{' '}
                    {provider.mapping_version} · observed_at_ms{' '}
                    {provider.observed_at_ms === null ? 'null' : provider.observed_at_ms}
                  </small>
                </li>
              );
            })}
          </ol>
        </li>
      ))}
    </ol>
  );
}

function VersionsTab({ evidence }: { evidence: CloudDocumentQueryResponse }) {
  const pins = evidence.version_pins;
  const versions = [
    ['Query', evidence.query_id],
    ['Workspace', evidence.workspace_id],
    ['Catalog', pins.catalog_version],
    ['Schema', pins.schema_version],
    ['Schema checksum', pins.schema_checksum],
    ['Physical schema', pins.source_schema_version],
    ['Physical schema checksum', pins.source_schema_checksum],
    ['Mapping', pins.mapping_version],
    ['Mapping checksum', pins.mapping_checksum],
    ['Connector', pins.connector_version],
    ['Source', pins.source_version],
    ['Source checksum', pins.source_checksum],
    ['Policy', pins.policy_version],
    ['Policy checksum', pins.policy_checksum],
  ] as const;
  return (
    <dl className="document-schema">
      {versions.map(([label, value]) => (
        <div key={label}>
          <dt>{label}</dt>
          <dd className="lens-mono">{value}</dd>
        </div>
      ))}
    </dl>
  );
}
