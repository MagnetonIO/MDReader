/**
 * **View As** and **Schema View**, together — because they are one object seen
 * two ways: a schema's *shape* (its entities, arrows and laws) and its
 * *contents* (the corpus projected through it).
 *
 * This is the thesis made operable. The same documents are on disk throughout;
 * choosing a lens changes only which typed reading of them the app shows. The
 * sheet says so in as many words, because a reader who thinks a lens rewrote
 * their notes has misunderstood the entire system.
 *
 * Honest rendering here means two things in particular:
 *   - the shape comes from the domain registration and the version/checksum
 *     from the persisted catalog, so a schema can be fully described while
 *     carrying no version at all. The two are shown as separate readings, and
 *     an unassigned version says "not yet versioned" rather than showing `v0`.
 *   - a projected relation carries no id and no provenance, because storage
 *     records only `(arrow, source, target)`. No citation affordance is drawn
 *     for one; the panel states where relation provenance actually lives.
 */

import { useCallback, useEffect, useState } from 'react';

import type {
  Projection,
  SchemaDetail,
  SchemaSummary,
  SemanticsAdapter,
} from '../../platform/semantics.js';
import {
  MORPHISM_PROVENANCE_NOTE,
  UNVERSIONED_REASON,
  checksumLabel,
  citeProvenance,
  lawSentence,
  localName,
  pathChain,
  plural,
  projectionSummary,
  registrationSummary,
  shortId,
  summarizeValue,
  versionLabel,
} from './lens-format.js';

export interface LensSheetProps {
  adapter: SemanticsAdapter;
  /** The schema the reader is currently interpreted through, or `null`. */
  activeSchema: string | null;
  /** Adopt a lens (or `null` for the plain reading). */
  onChoose: (schema: string | null) => void;
  /** Jump the reader to an object's cited lines. */
  onCite: (file: string, startLine: number, endLine: number) => void;
  onClose: () => void;
}

type Pane = 'shape' | 'contents';
type Status = 'loading' | 'ready' | 'error';

export function LensSheet({ adapter, activeSchema, onChoose, onCite, onClose }: LensSheetProps) {
  const [schemas, setSchemas] = useState<SchemaSummary[]>([]);
  const [status, setStatus] = useState<Status>('loading');
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(activeSchema);
  const [pane, setPane] = useState<Pane>('shape');

  useEffect(() => {
    let live = true;
    adapter
      .listSchemas()
      .then((found) => {
        if (!live) return;
        setSchemas(found);
        setStatus('ready');
        // Land on something inspectable: the active lens, else the first schema.
        setSelected((current) => current ?? found[0]?.name ?? null);
      })
      .catch((cause: unknown) => {
        if (!live) return;
        setError(cause instanceof Error ? cause.message : String(cause));
        setStatus('error');
      });
    return () => {
      live = false;
    };
  }, [adapter]);

  const summary = schemas.find((schema) => schema.name === selected) ?? null;

  return (
    <div
      className="overlay-backdrop"
      role="presentation"
      onMouseDown={onClose}
      onKeyDown={(event) => {
        if (event.key === 'Escape') onClose();
      }}
    >
      <div
        className="lens-sheet"
        role="dialog"
        aria-label="View as"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <header className="lens-sheet__head">
          <h2 className="lens-sheet__title">View as</h2>
          <p className="lens-sheet__lede">
            One corpus, several typed readings. A lens changes how the notes are interpreted — the
            files on disk are never touched.
          </p>
        </header>

        {status === 'error' ? (
          <p className="lens-sheet__empty">Could not read the schemas — {error}</p>
        ) : status === 'loading' ? (
          <p className="lens-sheet__empty">Reading the registered schemas…</p>
        ) : (
          <div className="lens-sheet__grid">
            <nav className="lens-sheet__schemas" aria-label="Registered schemas">
              <button
                type="button"
                className={`lens-choice${activeSchema === null ? ' lens-choice--active' : ''}`}
                onClick={() => onChoose(null)}
              >
                <span className="lens-choice__name">Plain reading</span>
                <span className="lens-choice__counts">no interpretation applied</span>
              </button>
              {schemas.map((schema) => (
                <button
                  key={schema.name}
                  type="button"
                  className={`lens-choice${schema.name === selected ? ' lens-choice--selected' : ''}${
                    schema.name === activeSchema ? ' lens-choice--active' : ''
                  }`}
                  aria-pressed={schema.name === activeSchema}
                  onClick={() => setSelected(schema.name)}
                  onDoubleClick={() => onChoose(schema.name)}
                >
                  <span className="lens-choice__name lens-mono">{schema.name}</span>
                  <span className="lens-choice__counts">
                    {schema.objectCount.toLocaleString()} in the graph
                    {schema.approvalRequired ? ` · ${plural(schema.proposalCount, 'claim')}` : ''}
                  </span>
                </button>
              ))}
            </nav>

            <section className="lens-sheet__detail">
              {summary === null ? (
                <p className="lens-sheet__empty">No schemas are registered for this workspace.</p>
              ) : (
                <>
                  <div className="lens-sheet__detail-head">
                    <h3 className="lens-sheet__schema lens-mono">{summary.name}</h3>
                    <div className="lens-sheet__panes" role="tablist" aria-label="Schema view">
                      <button
                        type="button"
                        role="tab"
                        aria-selected={pane === 'shape'}
                        className={`pane-tab${pane === 'shape' ? ' pane-tab--on' : ''}`}
                        onClick={() => setPane('shape')}
                      >
                        Shape
                      </button>
                      <button
                        type="button"
                        role="tab"
                        aria-selected={pane === 'contents'}
                        className={`pane-tab${pane === 'contents' ? ' pane-tab--on' : ''}`}
                        onClick={() => setPane('contents')}
                      >
                        Contents
                      </button>
                    </div>
                    <button
                      type="button"
                      className="btn-quiet"
                      onClick={() => onChoose(summary.name)}
                      disabled={summary.name === activeSchema}
                    >
                      {summary.name === activeSchema ? 'Reading as this' : 'Read as this'}
                    </button>
                  </div>

                  <SchemaCatalog summary={summary} />

                  {pane === 'shape' ? (
                    <ShapePane adapter={adapter} schema={summary.name} />
                  ) : (
                    <ContentsPane adapter={adapter} schema={summary.name} onCite={onCite} />
                  )}
                </>
              )}
            </section>
          </div>
        )}
      </div>
    </div>
  );
}

/**
 * The two independent readings of a schema's identity, kept visibly apart: what
 * the domain registration declares, and what the persisted catalog has recorded.
 * Before an index exists the second is genuinely empty, and this says why.
 */
function SchemaCatalog({ summary }: { summary: SchemaSummary }) {
  const unversioned = summary.version === null;
  return (
    <dl className="catalog">
      <div className="catalog__row">
        <dt>Registered</dt>
        <dd>{registrationSummary(summary)}</dd>
      </div>
      <div className="catalog__row">
        <dt>Catalog</dt>
        <dd className={unversioned ? 'catalog__absent' : undefined} title={unversioned ? UNVERSIONED_REASON : undefined}>
          <span className="lens-mono">{versionLabel(summary.version)}</span>
          {' · '}
          <span className="lens-mono">{checksumLabel(summary.checksum)}</span>
        </dd>
      </div>
      {unversioned ? (
        <div className="catalog__row catalog__row--note">
          <dt />
          <dd>{UNVERSIONED_REASON}</dd>
        </div>
      ) : null}
    </dl>
  );
}

function ShapePane({ adapter, schema }: { adapter: SemanticsAdapter; schema: string }) {
  const [detail, setDetail] = useState<SchemaDetail | null>(null);
  const [status, setStatus] = useState<Status>('loading');
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    setStatus('loading');
    adapter
      .describeSchema(schema)
      .then((found) => {
        if (!live) return;
        setDetail(found);
        setStatus('ready');
      })
      .catch((cause: unknown) => {
        if (!live) return;
        setError(cause instanceof Error ? cause.message : String(cause));
        setStatus('error');
      });
    return () => {
      live = false;
    };
  }, [adapter, schema]);

  if (status === 'error') return <p className="lens-sheet__empty">Could not read the schema — {error}</p>;
  if (status === 'loading' || !detail) return <p className="lens-sheet__empty">Reading the schema…</p>;

  return (
    <div className="shape">
      <section className="shape__section">
        <h4 className="shape__heading">Entities</h4>
        {detail.entities.length === 0 ? (
          <p className="shape__empty">This schema declares no entities.</p>
        ) : (
          <ul className="shape__entities">
            {detail.entities.map((entity) => (
              <li key={entity.name} className="entity">
                <span className="entity__name">{entity.name}</span>
                {entity.attributes.length === 0 ? (
                  <span className="entity__none">no attributes</span>
                ) : (
                  <ul className="entity__attrs">
                    {entity.attributes.map((attribute) => (
                      <li key={attribute.name} className="attr">
                        <span className="attr__name lens-mono">{attribute.name}</span>
                        <span className="attr__type lens-mono">{attribute.ty}</span>
                        {attribute.nullable ? <span className="attr__opt">optional</span> : null}
                      </li>
                    ))}
                  </ul>
                )}
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="shape__section">
        <h4 className="shape__heading">Arrows</h4>
        {detail.arrows.length === 0 ? (
          <p className="shape__empty">This schema declares no arrows.</p>
        ) : (
          <ul className="shape__arrows">
            {detail.arrows.map((arrow) => (
              <li key={`${arrow.dom}.${arrow.name}`} className="arrow">
                <span className="arrow__dom">{arrow.dom}</span>
                <span className="arrow__name lens-mono">{arrow.name}</span>
                <span className="arrow__cod">{arrow.cod}</span>
              </li>
            ))}
          </ul>
        )}
      </section>

      <section className="shape__section">
        <h4 className="shape__heading">Laws</h4>
        {detail.equations.length === 0 ? (
          <p className="shape__empty">This schema imposes no path equations.</p>
        ) : (
          <ul className="shape__laws">
            {detail.equations.map((equation) => (
              <li key={equation.law} className="law">
                {/* Said in words first: a law nobody can read is a law nobody checks. */}
                <p className="law__sentence">{lawSentence(equation)}</p>
                <div className="law__paths lens-mono">
                  <span>{pathChain(equation.start, equation.lhs)}</span>
                  <span className="law__eq">=</span>
                  <span>{pathChain(equation.start, equation.rhs)}</span>
                </div>
                {/* And kept verbatim, so the translation above never stands alone. */}
                <p className="law__kernel lens-mono" title="As the kernel renders it">
                  {equation.law}
                </p>
              </li>
            ))}
          </ul>
        )}
      </section>

      {detail.typeside.length > 0 ? (
        <section className="shape__section">
          <h4 className="shape__heading">Typeside</h4>
          <p className="shape__typeside lens-mono">{detail.typeside.join(' · ')}</p>
        </section>
      ) : null}
    </div>
  );
}

function ContentsPane({
  adapter,
  schema,
  onCite,
}: {
  adapter: SemanticsAdapter;
  schema: string;
  onCite: (file: string, startLine: number, endLine: number) => void;
}) {
  const [projection, setProjection] = useState<Projection | null>(null);
  const [status, setStatus] = useState<Status>('loading');
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    setStatus('loading');
    adapter
      .projectCorpus(schema)
      .then((found) => {
        if (!live) return;
        setProjection(found);
        setStatus('ready');
      })
      .catch((cause: unknown) => {
        if (!live) return;
        setError(cause instanceof Error ? cause.message : String(cause));
        setStatus('error');
      });
    return () => {
      live = false;
    };
  }, [adapter, schema]);

  const cite = useCallback(
    (file: string | null, start: number | null, end: number | null) => {
      if (file === null || start === null) return;
      onCite(file, start, end ?? start);
    },
    [onCite],
  );

  if (status === 'error') return <p className="lens-sheet__empty">Could not project the corpus — {error}</p>;
  if (status === 'loading' || !projection) return <p className="lens-sheet__empty">Projecting the corpus…</p>;

  const byArrow = new Map<string, typeof projection.morphisms>();
  for (const edge of projection.morphisms) {
    const bucket = byArrow.get(edge.arrow);
    if (bucket) bucket.push(edge);
    else byArrow.set(edge.arrow, [edge]);
  }

  return (
    <div className="contents">
      <p className="contents__summary">
        {projectionSummary({
          objects: projection.objects.length,
          morphisms: projection.morphisms.length,
          truncated: projection.truncated,
        })}
        {projection.truncated ? ' — the projection holds more than is shown here.' : ''}
      </p>

      {projection.objects.length === 0 ? (
        <p className="lens-sheet__empty">
          Nothing has been committed to this schema yet. Approve claims in the Inspector to populate it.
        </p>
      ) : (
        <ul className="contents__objects">
          {projection.objects.map((object) => {
            const citable = object.provenance.file !== null && object.provenance.startLine !== null;
            return (
              <li key={object.id} className="projected">
                <span className="projected__entity">{localName(object.entity)}</span>
                <span className="projected__value">{summarizeValue(object.value) ?? object.value}</span>
                {citable ? (
                  <button
                    type="button"
                    className="projected__cite lens-mono"
                    title="Go to the cited lines"
                    onClick={() =>
                      cite(object.provenance.file, object.provenance.startLine, object.provenance.endLine)
                    }
                  >
                    {citeProvenance(object.provenance)}
                  </button>
                ) : (
                  <span className="projected__cite lens-mono">{citeProvenance(object.provenance)}</span>
                )}
              </li>
            );
          })}
        </ul>
      )}

      {projection.morphisms.length > 0 ? (
        <section className="contents__edges">
          <h4 className="shape__heading">Relations</h4>
          {[...byArrow.entries()].map(([arrow, edges]) => (
            <div key={arrow} className="edge-group">
              <div className="edge-group__arrow lens-mono">{localName(arrow)}</div>
              <ul className="edge-group__list">
                {edges.map((edge) => (
                  <li key={`${edge.arrow}:${edge.source}:${edge.target}`} className="edge lens-mono">
                    <span title={edge.source}>{shortId(edge.source)}</span>
                    <span className="edge__to">→</span>
                    <span title={edge.target}>{shortId(edge.target)}</span>
                  </li>
                ))}
              </ul>
            </div>
          ))}
          {/* No citation button here, and no disabled one either: there is
              nothing to cite, and the panel says where the answer does live. */}
          <p className="contents__note">{MORPHISM_PROVENANCE_NOTE}</p>
        </section>
      ) : null}
    </div>
  );
}
