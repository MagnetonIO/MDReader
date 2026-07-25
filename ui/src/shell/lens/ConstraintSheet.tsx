/**
 * The Constraint Explorer: every registered schema's path equations run over
 * the committed graph, and each failure shown **where it happened**.
 *
 * The engine goes to real trouble to locate a violation — schema, law, both
 * composed paths, the offending object, and the markdown span it was derived
 * from. Re-rendering that as prose ("some decisions are inconsistent") would
 * throw away the only part that lets somebody fix it. So every located
 * violation here carries its span verbatim and jumps the reader straight to
 * those lines; a span is never rounded, summarised, or replaced by a document
 * link.
 *
 * A law is stated twice: once as a sentence a person can check against their
 * own notes, and once as the kernel renders it, so the translation is auditable.
 */

import { useEffect, useState } from 'react';

import type { ConstraintReport, SemanticsAdapter, SourceSpan, ViolatingObject } from '../../platform/semantics.js';
import { citeSourceSpan, lawSentence, localName, pathChain, shortHash, shortId, violationSentence } from './lens-format.js';

export interface ConstraintSheetProps {
  adapter: SemanticsAdapter;
  /**
   * Open the document a violation names, at exactly the lines it names. The
   * span is handed over whole — the caller resolves the repo-relative file to a
   * path the reader can open, and reports it honestly when it cannot.
   */
  onJump: (span: SourceSpan) => void;
  onClose: () => void;
}

type Status = 'loading' | 'ready' | 'error';

export function ConstraintSheet({ adapter, onJump, onClose }: ConstraintSheetProps) {
  const [report, setReport] = useState<ConstraintReport | null>(null);
  const [status, setStatus] = useState<Status>('loading');
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    adapter
      .checkConstraints()
      .then((found) => {
        if (!live) return;
        setReport(found);
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
  }, [adapter]);

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
        className="lens-sheet lens-sheet--laws"
        role="dialog"
        aria-label="Constraint explorer"
        onMouseDown={(event) => event.stopPropagation()}
      >
        <header className="lens-sheet__head">
          <h2 className="lens-sheet__title">Where the laws break</h2>
          <p className="lens-sheet__lede">
            {status === 'ready' && report
              ? report.violations.length === 0
                ? `Every law holds across ${report.schemasChecked} ${
                    report.schemasChecked === 1 ? 'schema' : 'schemas'
                  }.`
                : `${report.violations.length} ${
                    report.violations.length === 1 ? 'violation' : 'violations'
                  } across ${report.schemasChecked} ${report.schemasChecked === 1 ? 'schema' : 'schemas'}. Each one names the lines it came from.`
              : 'Running every registered schema’s laws over the graph.'}
          </p>
        </header>

        {status === 'error' ? (
          <p className="lens-sheet__empty">Could not check the constraints — {error}</p>
        ) : status === 'loading' || !report ? (
          <p className="lens-sheet__empty">Checking…</p>
        ) : report.violations.length === 0 ? (
          <p className="lens-sheet__empty">
            Nothing to fix. The committed graph satisfies every path equation its schemas impose.
          </p>
        ) : (
          <ol className="violations">
            {report.violations.map((violation, index) => (
              <li
                key={`${violation.schema}:${violation.law}:${violation.subject.id}:${index}`}
                className="violation"
              >
                <div className="violation__law">
                  <span className="violation__schema lens-mono">{violation.schema}</span>
                  <p className="law__sentence">
                    {lawSentence({
                      start: localName(violation.subject.entity),
                      lhs: violation.lhsPath,
                      rhs: violation.rhsPath,
                    })}
                  </p>
                  <div className="law__paths lens-mono">
                    <span>{pathChain(localName(violation.subject.entity), violation.lhsPath)}</span>
                    <span className="law__eq">=</span>
                    <span>{pathChain(localName(violation.subject.entity), violation.rhsPath)}</span>
                  </div>
                  <p className="law__kernel lens-mono" title="As the kernel renders it">
                    {violation.law}
                  </p>
                </div>

                <p className="violation__what">{violationSentence(violation)}</p>

                <div className="violation__objects">
                  <LocatedObject label="Subject" object={violation.subject} onJump={onJump} primary />
                  <LocatedObject label="Left path lands on" object={violation.lhs} onJump={onJump} />
                  <LocatedObject label="Right path lands on" object={violation.rhs} onJump={onJump} />
                </div>
              </li>
            ))}
          </ol>
        )}
      </div>
    </div>
  );
}

function LocatedObject({
  label,
  object,
  onJump,
  primary = false,
}: {
  label: string;
  object: ViolatingObject | null;
  onJump: (span: SourceSpan) => void;
  primary?: boolean;
}) {
  if (object === null) {
    return (
      <div className="located located--missing">
        <span className="located__label">{label}</span>
        <span className="located__none">nowhere — the arrow is missing</span>
      </div>
    );
  }
  const span = object.source;
  const jumpable = span !== null && span.startLine !== null;
  return (
    <div className={`located${primary ? ' located--primary' : ''}`}>
      <span className="located__label">{label}</span>
      <span className="located__entity">{localName(object.entity)}</span>
      <span className="located__id lens-mono" title={object.id}>
        {shortId(object.id)}
      </span>
      {jumpable ? (
        <button
          type="button"
          className="located__jump lens-mono"
          title="Open the document at these lines"
          // The span goes over whole: file, first line, last line, hash. The
          // caller opens exactly what the engine located, not an approximation.
          onClick={() => onJump(span)}
        >
          {citeSourceSpan(span)}
        </button>
      ) : (
        <span className="located__id lens-mono">{citeSourceSpan(span)}</span>
      )}
      {span?.contentHash ? <span className="located__hash lens-mono">{shortHash(span.contentHash)}</span> : null}
    </div>
  );
}
