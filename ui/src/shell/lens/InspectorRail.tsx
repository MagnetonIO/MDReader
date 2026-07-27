/**
 * The Semantic Inspector: select a passage in the reader, see what the
 * extractor derived from it — and rule on it.
 *
 * The surface is built as a **critical apparatus**, not a data grid. Each entry
 * is a *claim* set in the reading face, with its citation beneath it in mono
 * (file, line range, content hash, schema, extractor, rule, confidence) and two
 * quiet verbs to rule with. That is the honest description of what extraction
 * produces: an editor proposing a reading of a passage, and a human who decides.
 * Nothing here is phrased as established fact.
 *
 * Three truths this panel refuses to smooth over:
 *   - a claim's recorded ruling and its actual graph membership are read
 *     separately, and shown together when they disagree (there is no overlay
 *     GC, so `proposed` + `in graph` is a real and meaningful state);
 *   - a rejection whose cited text has since changed is surfaced as an explicit
 *     invitation to re-review, so un-rejecting stays deliberate;
 *   - a review is signed, and the app has no identity to sign with, so it asks.
 *
 * After a ruling the span is re-read rather than patched in place: whether a
 * promotion actually landed is a fact about storage, and guessing at it here
 * would be the exact dishonesty the panel exists to prevent.
 */

import { useCallback, useEffect, useRef, useState } from 'react';

import type {
  FederatedDocument,
  InspectedFact,
  SemanticsAdapter,
  SpanSelection,
} from '../../platform/semantics.js';
import {
  STATE_LABEL,
  citeProvenance,
  claimFields,
  claimReading,
  confidenceLabel,
  factSchemaLabel,
  factStanding,
  reReviewHashes,
  reReviewPrompt,
  shortHash,
} from './lens-format.js';
import { DocumentInspector } from './DocumentInspector.js';

export interface InspectorRailProps {
  /** The semantic transport. */
  adapter: SemanticsAdapter;
  /** Whether the rail is expanded. */
  open: boolean;
  /** Expand / collapse. */
  onToggle: () => void;
  /** The passage under inspection, or `null` when nothing is selected. */
  selection: SpanSelection | null;
  /** The name reviews are signed with; empty when no reviewer is set. */
  reviewer: string;
  /** Adopt a reviewer name (trimmed by the store). */
  onSetReviewer: (name: string) => void;
  /** Jump the reader to a claim's cited lines. */
  onCite: (file: string, startLine: number, endLine: number) => void;
  /** The active workspace root; a change drops any in-flight read. */
  workspaceKey: string | null;
  /** The backend epoch generation, including same-root reopens. */
  workspaceGeneration?: number;
  /** Active Markdown document queried across filesystem, Git, and frontmatter. */
  documentPath?: string | null;
  /** Native federated query seam. Optional only for isolated legacy tests. */
  queryDocument?: (
    path: string,
    invocation: string,
    request: string,
  ) => Promise<FederatedDocument>;
  /** Cancels superseded native federation work for this Inspector instance. */
  cancelDocument?: (invocation: string, request: string) => Promise<void>;
}

type Status = 'idle' | 'loading' | 'ready' | 'error';

export function InspectorRail({
  adapter,
  open,
  onToggle,
  selection,
  reviewer,
  onSetReviewer,
  onCite,
  workspaceKey,
  workspaceGeneration = 0,
  documentPath = null,
  queryDocument,
  cancelDocument,
}: InspectorRailProps) {
  const [facts, setFacts] = useState<InspectedFact[]>([]);
  const [status, setStatus] = useState<Status>('idle');
  const [error, setError] = useState<string | null>(null);
  const [ruling, setRuling] = useState<string | null>(null);
  const [signing, setSigning] = useState(false);
  const [draftName, setDraftName] = useState('');
  const reqRef = useRef(0);

  const path = selection?.path ?? null;
  const startLine = selection?.startLine ?? null;
  const endLine = selection?.endLine ?? null;

  const load = useCallback(async () => {
    if (!path || startLine === null || endLine === null) {
      setFacts([]);
      setStatus('idle');
      return;
    }
    const id = (reqRef.current += 1);
    setStatus('loading');
    try {
      const found = await adapter.inspectSpan({ path, startLine, endLine });
      // Superseded by a newer selection or a workspace switch.
      if (reqRef.current !== id) return;
      setFacts(found);
      setError(null);
      setStatus('ready');
    } catch (cause) {
      if (reqRef.current !== id) return;
      setFacts([]);
      setError(cause instanceof Error ? cause.message : String(cause));
      setStatus('error');
    }
  }, [adapter, path, startLine, endLine]);

  useEffect(() => {
    if (!open) {
      // Abandon any in-flight read: a collapsed rail must not repopulate itself.
      reqRef.current += 1;
      setFacts([]);
      setStatus('idle');
      return;
    }
    void load();
  }, [open, load, workspaceKey]);

  const rule = useCallback(
    async (fact: InspectedFact, verdict: 'approve' | 'reject') => {
      const signature = reviewer.trim();
      // The gate, restated at the call site: with no reviewer there is nothing
      // to sign with, and the app will not invent one.
      if (signature.length === 0) {
        setSigning(true);
        return;
      }
      setRuling(fact.factId);
      try {
        if (verdict === 'approve') await adapter.approveFact(fact.factId, signature);
        else await adapter.rejectFact(fact.factId, signature);
        await load();
      } catch (cause) {
        setError(cause instanceof Error ? cause.message : String(cause));
        setStatus('error');
      } finally {
        setRuling(null);
      }
    },
    [adapter, reviewer, load],
  );

  if (!open) {
    return (
      <button
        type="button"
        className="lens-strip"
        onClick={onToggle}
        title="Show the semantic inspector"
        aria-expanded={false}
      >
        <span className="lens-strip__label">Inspector</span>
      </button>
    );
  }

  const signed = reviewer.trim().length > 0;
  const canSubmitName = draftName.trim().length > 0;

  return (
    <aside className="lens-rail" aria-label="Semantic inspector">
      <div className="lens-rail__head">
        <span className="lens-rail__title">Inspector</span>
        <button
          type="button"
          className="lens-rail__close"
          onClick={onToggle}
          title="Hide the semantic inspector"
          aria-expanded
          aria-label="Hide the semantic inspector"
        >
          ×
        </button>
      </div>

      <div className="lens-rail__body">
        {documentPath && queryDocument && cancelDocument ? (
          <>
            <DocumentInspector
              path={documentPath}
              query={queryDocument}
              cancel={cancelDocument}
              workspaceKey={workspaceKey}
              workspaceGeneration={workspaceGeneration}
            />
            <div className="inspector__section-break" />
          </>
        ) : null}
        <p className="inspector__scope">
          {selection ? (
            <>
              Reading claims derived from{' '}
              <span className="lens-mono">
                {selection.endLine === selection.startLine
                  ? `line ${selection.startLine}`
                  : `lines ${selection.startLine}–${selection.endLine}`}
              </span>
              .
            </>
          ) : (
            'Select a passage in the reader to see what was derived from it.'
          )}
        </p>

        {/* The signature block. Always visible, so who is ruling is never hidden. */}
        {signed && !signing ? (
          <p className="inspector__signature">
            Reviews are signed <span className="inspector__signer">{reviewer}</span>
            <button
              type="button"
              className="link-btn"
              onClick={() => {
                setDraftName(reviewer);
                setSigning(true);
              }}
            >
              Change
            </button>
          </p>
        ) : (
          <form
            className="inspector__sign"
            onSubmit={(event) => {
              event.preventDefault();
              if (!canSubmitName) return;
              onSetReviewer(draftName);
              setSigning(false);
            }}
          >
            <label className="inspector__sign-label" htmlFor="inspector-reviewer">
              Reviews are signed. Who is ruling on these claims?
            </label>
            <div className="inspector__sign-row">
              <input
                id="inspector-reviewer"
                className="inspector__sign-input"
                type="text"
                value={draftName}
                placeholder="Your name"
                autoComplete="name"
                onChange={(event) => setDraftName(event.target.value)}
              />
              <button type="submit" className="btn-quiet" disabled={!canSubmitName}>
                Save
              </button>
            </div>
          </form>
        )}

        {status === 'error' ? (
          <p className="lens-rail__empty">Could not read this passage — {error}</p>
        ) : status === 'loading' ? (
          <p className="lens-rail__empty">Reading the passage…</p>
        ) : !selection ? null : facts.length === 0 ? (
          <p className="lens-rail__empty">
            Nothing was derived from these lines. The passage is read as prose only.
          </p>
        ) : (
          <ol className="claims">
            {facts.map((fact) => (
              <ClaimCard
                key={fact.factId}
                fact={fact}
                busy={ruling === fact.factId}
                canRule={signed}
                onApprove={() => void rule(fact, 'approve')}
                onReject={() => void rule(fact, 'reject')}
                onCite={onCite}
              />
            ))}
          </ol>
        )}
      </div>
    </aside>
  );
}

interface ClaimCardProps {
  fact: InspectedFact;
  busy: boolean;
  canRule: boolean;
  onApprove: () => void;
  onReject: () => void;
  onCite: (file: string, startLine: number, endLine: number) => void;
}

function ClaimCard({ fact, busy, canRule, onApprove, onReject, onCite }: ClaimCardProps) {
  const reading = claimReading(fact);
  const standing = factStanding(fact);
  const rereview = reReviewPrompt(fact);
  const fields = reading.kind === 'object' ? claimFields(fact.value) : [];
  const { provenance } = fact;
  const hashes = reReviewHashes(fact, provenance);
  const citation = citeProvenance(provenance);
  const canCite = provenance.file !== null && provenance.startLine !== null;

  return (
    <li className={`claim claim--${standing.state}`} data-testid={`claim-${fact.factId}`}>
      <div className="claim__standing">
        <span className={`claim__state claim__state--${standing.state}`}>
          {STATE_LABEL[standing.state]}
        </span>
        {/* Read from storage, not inferred: both readings appear, always. */}
        <span className={`claim__graph${standing.inGraph ? ' claim__graph--in' : ''}`}>
          {standing.inGraph ? 'in the graph' : 'not in the graph'}
        </span>
        {fact.reviewer ? <span className="claim__reviewer">by {fact.reviewer}</span> : null}
      </div>

      <p className="claim__sentence">{reading.sentence}</p>

      {fields.length > 0 ? (
        <dl className="claim__fields">
          {fields.map((field) => (
            <div key={field.key} className="claim__field">
              <dt>{field.key}</dt>
              <dd>{field.value}</dd>
            </div>
          ))}
        </dl>
      ) : null}

      {standing.divergent ? <p className="claim__note">{standing.note}</p> : null}
      {rereview ? (
        <p className="claim__note claim__note--rereview">
          {rereview}
          {/* Both sides, always. A digest the engine did not record is named as
              missing rather than dropped: showing one hash alone would read as a
              comparison this panel is in no position to have made. */}
          <span className="claim__hashes lens-mono">
            {hashes.reviewed} · {hashes.current}
          </span>
          {hashes.uncomparable ? (
            <span className="claim__hashes-why">{hashes.uncomparable}</span>
          ) : null}
        </p>
      ) : null}

      <div className="claim__cite lens-mono">
        {canCite ? (
          <button
            type="button"
            className="claim__cite-link"
            title="Go to the cited lines"
            onClick={() => onCite(provenance.file!, provenance.startLine!, provenance.endLine ?? provenance.startLine!)}
          >
            {citation}
          </button>
        ) : (
          <span>{citation}</span>
        )}
        {provenance.contentHash ? <span> · {shortHash(provenance.contentHash)}</span> : null}
        <br />
        <span>{factSchemaLabel(fact.schema, provenance.schemaVersion)}</span>
        <span> · {provenance.extractor ?? fact.extractor}</span>
        <span> · rule {fact.rule}</span>
        <span> · {confidenceLabel(provenance.confidence)}</span>
      </div>

      <div className="claim__rule">
        <button
          type="button"
          className="btn-quiet btn-quiet--approve"
          onClick={onApprove}
          disabled={busy || !canRule}
          title={canRule ? 'Accept this claim into the graph' : 'Add your name above to rule on claims'}
        >
          Approve
        </button>
        <button
          type="button"
          className="btn-quiet btn-quiet--reject"
          onClick={onReject}
          disabled={busy || !canRule}
          title={canRule ? 'Rule this claim out' : 'Add your name above to rule on claims'}
        >
          Reject
        </button>
      </div>
    </li>
  );
}
