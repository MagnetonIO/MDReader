import { useId, useRef, useState } from 'react';

import type {
  EntityRow,
  FieldObservation,
  FieldProvenance,
  LibraryEntityQueryResponse_Serialize,
} from '../bindings.js';

type InspectorTab = 'result' | 'sources' | 'provenance' | 'schema';

const TABS: Array<{ id: InspectorTab; label: string }> = [
  { id: 'result', label: 'Result' },
  { id: 'sources', label: 'Sources' },
  { id: 'provenance', label: 'Provenance' },
  { id: 'schema', label: 'Schema' },
];

export interface LibrarySemanticInspectorProps {
  evidence: LibraryEntityQueryResponse_Serialize;
  row: EntityRow;
}

function valueText(value: unknown): string {
  if (value === null) return 'null';
  if (typeof value === 'string') return value;
  return JSON.stringify(value) ?? String(value);
}

function observationLabel(observation: FieldObservation): string {
  switch (observation.state.state) {
    case 'observed_value':
      return `observed ${valueText(observation.state.value)}`;
    case 'observed_null':
      return 'observed null';
    case 'field_unseen':
      return 'field not observed';
    case 'record_unseen':
      return 'record not observed';
    case 'not_asked':
      return `not asked · ${observation.state.value.reason}`;
  }
}

function selectedObservation(field: FieldProvenance): FieldObservation | null {
  if (field.selected === null) return null;
  return field.considered[field.selected] ?? null;
}

export function LibrarySemanticInspector({
  evidence,
  row,
}: LibrarySemanticInspectorProps) {
  const [tab, setTab] = useState<InspectorTab>('result');
  const tabsetId = useId();
  const tabRefs = useRef(new Map<InspectorTab, HTMLButtonElement>());
  const complete = evidence.answer.completeness.state === 'complete';

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
    <section className="document-inspector" aria-label="Document evidence">
      <div className="document-inspector__intro">
        <span className="document-inspector__eyebrow">Library document</span>
        <span
          className={`document-inspector__completeness document-inspector__completeness--${
            complete ? 'complete' : 'partial'
          }`}
        >
          {complete ? 'Complete' : 'Partial'}
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
            <ProvenanceTab evidence={evidence} row={row} />
          ) : null}
          {tab === item.id && item.id === 'schema' ? (
            <SchemaTab evidence={evidence} />
          ) : null}
        </div>
      ))}
    </section>
  );
}

function ResultTab({ row }: { row: EntityRow }) {
  return (
    <>
      <p className="document-inspector__path lens-mono">{row.record_key}</p>
      <dl className="document-result">
        {Object.entries(row.provenance).map(([name, field]) => {
          const selected = selectedObservation(field);
          const value = row.fields[name];
          return (
            <div key={name}>
              <dt>{name}</dt>
              <dd>
                <span className={selected ? undefined : 'is-absent'}>
                  {value === undefined ? 'Not observed' : valueText(value)}
                </span>
                <small>
                  {selected
                    ? `source ${selected.source_version_id}`
                    : 'No selected provider'}
                </small>
              </dd>
            </div>
          );
        })}
      </dl>
    </>
  );
}

function SourcesTab({ evidence }: { evidence: LibraryEntityQueryResponse_Serialize }) {
  return (
    <ol className="document-sources">
      {evidence.answer.sources.map((source) => {
        const member = evidence.memberships.find((candidate) =>
          candidate.source_ids.includes(source.source_id),
        );
        return (
          <li key={source.source_id}>
            <div className="document-sources__head">
              <strong>{member?.name ?? source.source_id}</strong>
              <span>{source.outcome}</span>
            </div>
            <p className="lens-mono">{source.source_id}</p>
            <p>
              {source.freshness_ms === null
                ? 'Freshness unavailable'
                : `${source.freshness_ms} ms freshness`}
            </p>
            <p>
              {source.scanned} scanned · {source.returned} returned
            </p>
          </li>
        );
      })}
    </ol>
  );
}

function ProvenanceTab({
  evidence,
  row,
}: {
  evidence: LibraryEntityQueryResponse_Serialize;
  row: EntityRow;
}) {
  const memberName = (observation: FieldObservation): string | null => {
    const sourceId = evidence.answer.sources.find(
      (source) => source.source_version_id === observation.source_version_id,
    )?.source_id;
    if (!sourceId) return null;
    return (
      evidence.memberships.find((member) => member.source_ids.includes(sourceId))?.name ??
      null
    );
  };
  return (
    <ol className="document-provenance">
      {Object.entries(row.provenance).map(([name, field]) => (
        <li key={name}>
          <div className="document-provenance__field">
            <strong>{name}</strong>
            <span>
              {field.selected === null ? 'No selected provider' : `selected ${field.selected}`}
            </span>
          </div>
          <ol>
            {field.considered.map((observation, index) => (
              <li
                key={`${observation.source_version_id}:${observation.record_key}:${index}`}
              >
                <span className="lens-mono">
                  {memberName(observation) ? `${memberName(observation)} · ` : null}
                  {observation.physical_attribute
                    ? `${observation.source_version_id}.${observation.physical_attribute}`
                    : observation.source_version_id}
                </span>
                <span>{observationLabel(observation)}</span>
                <small>
                  record {observation.record_key} · mapping {observation.mapping_version_id}
                </small>
              </li>
            ))}
          </ol>
        </li>
      ))}
    </ol>
  );
}

function SchemaTab({ evidence }: { evidence: LibraryEntityQueryResponse_Serialize }) {
  const pins = evidence.answer.pins;
  const versions = [
    ['Query', evidence.answer.query_id],
    ['Catalog authority', pins.catalog_resolver_id],
    ['Catalog', pins.catalog_revision_id],
    ['Schema', pins.schema_version_id],
    ['Physical schemas', pins.physical_schema_version_ids.join(', ')],
    ['Mappings', pins.mapping_version_ids.join(', ')],
    ['Sources', pins.source_version_ids.join(', ')],
    ['Policies', pins.policy_version_ids.join(', ')],
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
