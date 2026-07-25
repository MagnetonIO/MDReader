/**
 * How the semantic layer says things.
 *
 * Every rule about *honest rendering* lives here as a pure function, so it is
 * asserted directly rather than inferred from a screenshot:
 *
 *   - an unassigned schema version reads **"not yet versioned"** — never `v0`,
 *     never a blank cell;
 *   - a claim's recorded review state and its actual graph membership are two
 *     separate readings, and when they disagree BOTH are shown with the reason;
 *   - a law is stated as a sentence a person can check, with the kernel's own
 *     rendering kept alongside rather than replaced by it;
 *   - an absent number is absent, not zero.
 *
 * Nothing here formats a value the engine did not supply.
 */

import type {
  FactProvenance,
  InspectedFact,
  SchemaEquation,
  SourceSpan,
} from '../../platform/semantics.js';

/** `1 entity` / `3 entities` — a count and its noun, agreeing. */
export function plural(count: number, singular: string, pluralForm = `${singular}s`): string {
  return `${count.toLocaleString()} ${count === 1 ? singular : pluralForm}`;
}

// --- schema catalog -------------------------------------------------------

/**
 * How a schema's persisted version reads. `null` means the index has not been
 * built, so the engine has assigned no version — say that, rather than
 * inventing `v0` or leaving the reader to guess at an empty cell.
 */
export function versionLabel(version: number | null): string {
  return version === null ? 'not yet versioned' : `v${version}`;
}

/** Why a version can be missing while the shape is fully known. */
export const UNVERSIONED_REASON =
  'The shape comes from the domain registration. Versions and checksums are assigned when the index is built.';

/** How a schema's checksum reads, abbreviated but never faked. */
export function checksumLabel(checksum: string | null): string {
  return checksum === null ? 'no checksum yet' : shortHash(checksum);
}

/**
 * Abbreviate a content hash while keeping its algorithm prefix, so
 * `blake3:9f2c…4ab1` still reads as a blake3 digest and not as an opaque id.
 */
export function shortHash(hash: string): string {
  const colon = hash.indexOf(':');
  const algo = colon >= 0 ? hash.slice(0, colon + 1) : '';
  const digest = colon >= 0 ? hash.slice(colon + 1) : hash;
  if (digest.length <= 12) return hash;
  return `${algo}${digest.slice(0, 8)}…${digest.slice(-4)}`;
}

// --- provenance -----------------------------------------------------------

/**
 * A citation for a cited span: `notes/adr-3.md:12–18`. Degrades honestly —
 * a file with no line range cites the file, and no file at all says so.
 */
export function citeSpan(span: { file: string | null; startLine: number | null; endLine: number | null }): string {
  if (!span.file) return 'no source recorded';
  if (span.startLine === null) return span.file;
  if (span.endLine === null || span.endLine === span.startLine) {
    return `${span.file}:${span.startLine}`;
  }
  return `${span.file}:${span.startLine}–${span.endLine}`;
}

/** The citation for a fact's derivation record. */
export function citeProvenance(provenance: FactProvenance): string {
  return citeSpan(provenance);
}

/** The citation for a located violation's span. */
export function citeSourceSpan(span: SourceSpan | null): string {
  return span ? citeSpan(span) : 'no source recorded';
}

/**
 * Confidence as a percentage, or the honest absence. An extractor that
 * recorded no confidence has not implicitly recorded zero.
 */
export function confidenceLabel(confidence: number | null): string {
  return confidence === null ? 'no confidence recorded' : `${Math.round(confidence * 100)}% confident`;
}

/** A schema version stamped on a fact (a string on the wire, unlike the catalog's ordinal). */
export function factSchemaLabel(schema: string, version: string | null): string {
  return version === null ? `${schema} (unversioned)` : `${schema}@${version}`;
}

// --- names ----------------------------------------------------------------

/**
 * The schema-local tail of a qualified name: `decision_overlay.Decision` reads
 * as `Decision` once the schema is already named by its surrounding context.
 */
export function localName(qualified: string): string {
  const dot = qualified.lastIndexOf('.');
  return dot >= 0 ? qualified.slice(dot + 1) : qualified;
}

/** A uuid abbreviated for display; the full value stays available as a title. */
export function shortId(id: string): string {
  return id.length <= 10 ? id : `${id.slice(0, 8)}…`;
}

// --- claims ---------------------------------------------------------------

/** How a claim reads to a person about to rule on it. */
export interface ClaimReading {
  /** `object` claims assert a thing exists; `morphism` claims assert a relation. */
  kind: 'object' | 'morphism' | 'unknown';
  /** The claim in the reader's own words. */
  sentence: string;
}

/**
 * State a claim as a sentence. Deliberately phrased as something *proposed*:
 * the extractor says this passage names a Decision; whether it does is the
 * reader's call, not the machine's.
 */
export function claimReading(fact: InspectedFact): ClaimReading {
  if (fact.kind === 'morphism') {
    return {
      kind: 'morphism',
      sentence: `${shortId(fact.sourceId ?? '?')} ${localName(fact.name)} ${shortId(fact.targetId ?? '?')}`,
    };
  }
  if (fact.kind === 'object') {
    const summary = summarizeValue(fact.value);
    return {
      kind: 'object',
      sentence: summary ? `${localName(fact.name)} — ${summary}` : localName(fact.name),
    };
  }
  return { kind: 'unknown', sentence: localName(fact.name) };
}

/**
 * A one-line reading of an object's JSON payload: its most name-like field,
 * else the compact JSON itself. Never truncates silently below a readable
 * length — a claim a reviewer cannot read is a claim they cannot rule on.
 */
export function summarizeValue(value: string | null): string | null {
  if (value === null) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(value);
  } catch {
    return value;
  }
  if (parsed === null || typeof parsed !== 'object') return String(parsed);
  const record = parsed as Record<string, unknown>;
  for (const key of ['title', 'name', 'label', 'summary', 'text', 'id']) {
    const candidate = record[key];
    if (typeof candidate === 'string' && candidate.trim().length > 0) return candidate;
  }
  return value;
}

/** The attribute rows a reviewer sees under an object claim. */
export function claimFields(value: string | null): { key: string; value: string }[] {
  if (value === null) return [];
  let parsed: unknown;
  try {
    parsed = JSON.parse(value);
  } catch {
    return [];
  }
  if (parsed === null || typeof parsed !== 'object' || Array.isArray(parsed)) return [];
  return Object.entries(parsed as Record<string, unknown>).map(([key, raw]) => ({
    key,
    value: typeof raw === 'string' ? raw : JSON.stringify(raw),
  }));
}

// --- review standing ------------------------------------------------------

export type ReviewState = 'proposed' | 'approved' | 'rejected' | 'unknown';

/**
 * A claim's standing: what a human recorded, and what the graph actually
 * holds. These are two independent readings, and the panel shows both.
 */
export interface FactStanding {
  state: ReviewState;
  /** Read from storage, never inferred from {@link state}. */
  inGraph: boolean;
  /** True when the recorded ruling and graph membership disagree. */
  divergent: boolean;
  /**
   * Why they disagree, in plain terms. `null` when they agree — there is
   * nothing to explain and no banner to raise.
   */
  note: string | null;
}

function asReviewState(state: string): ReviewState {
  return state === 'proposed' || state === 'approved' || state === 'rejected' ? state : 'unknown';
}

/**
 * Read a claim's standing without smoothing it over.
 *
 * There is no overlay GC, so a claim can legitimately be `proposed` AND in the
 * graph: an earlier approval committed it, then the source changed and the
 * approval was reset. Both readings are true at once, and the panel says so
 * rather than picking whichever looks tidier.
 */
export function factStanding(fact: Pick<InspectedFact, 'state' | 'inGraph'>): FactStanding {
  const state = asReviewState(fact.state);
  const inGraph = fact.inGraph;
  if (state === 'proposed' && inGraph) {
    return {
      state,
      inGraph,
      divergent: true,
      note: 'An earlier approval committed this claim. The source has changed since, so it is proposed again — and the copy committed then is still in the graph.',
    };
  }
  if (state === 'rejected' && inGraph) {
    return {
      state,
      inGraph,
      divergent: true,
      note: 'Recorded as rejected, yet still committed to the graph. Reject it again to retract the committed copy.',
    };
  }
  if (state === 'approved' && !inGraph) {
    return {
      state,
      inGraph,
      divergent: true,
      note: 'Approved, but not in the graph — a relation is only committed once both of the objects it connects are approved.',
    };
  }
  return { state, inGraph, divergent: false, note: null };
}

/** How each recorded state reads in the panel. */
export const STATE_LABEL: Record<ReviewState, string> = {
  proposed: 'Proposed',
  approved: 'Approved',
  rejected: 'Rejected',
  unknown: 'Unrecognised state',
};

/**
 * The re-review prompt for a claim whose cited text moved under it. Only ever
 * raised for a preserved rejection: a re-import keeps the rejection rather than
 * silently re-proposing a claim a human ruled out, so un-rejecting stays a
 * deliberate act.
 */
export function reReviewPrompt(fact: Pick<InspectedFact, 'sourceChangedSinceReview' | 'state'>): string | null {
  if (!fact.sourceChangedSinceReview) return null;
  const ruling = asReviewState(fact.state) === 'rejected' ? 'You rejected this' : 'You reviewed this';
  return `${ruling}. The cited text has changed since — review it again?`;
}

/** The two digests a re-review prompt is checked against, and whether it can be. */
export interface ReReviewHashes {
  /** The digest as it read at review time — or the statement that none was. */
  reviewed: string;
  /** The digest as the cited text reads now — or the statement that none is. */
  current: string;
  /**
   * Why the two cannot be held against each other; `null` when both are real
   * digests and a reader can compare them themselves.
   */
  uncomparable: string | null;
}

/**
 * The hash pair beneath a re-review prompt.
 *
 * The prompt asserts that the cited text CHANGED, and the only thing that makes
 * that assertion checkable rather than merely stated is seeing both digests. So
 * a digest the engine did not record is NAMED as missing, with the reason, and
 * never quietly left out: one hash on its own reads as a comparison that was
 * made, when none could be. "These differ" and "we could not compare" are
 * different claims, and this makes exactly the one that is true.
 */
export function reReviewHashes(
  fact: Pick<InspectedFact, 'reviewedContentHash'>,
  provenance: Pick<FactProvenance, 'file' | 'contentHash'>,
): ReReviewHashes {
  const reviewed = fact.reviewedContentHash;
  const current = provenance.contentHash;
  return {
    reviewed: reviewed === null ? 'reviewed hash not recorded' : `reviewed ${shortHash(reviewed)}`,
    current: current === null ? 'current hash unavailable' : `now ${shortHash(current)}`,
    uncomparable:
      reviewed !== null && current !== null
        ? null
        : uncomparableReason(reviewed === null, current === null, provenance.file),
  };
}

function uncomparableReason(noReviewed: boolean, noCurrent: boolean, file: string | null): string {
  const why =
    noCurrent && noReviewed
      ? 'neither digest was recorded'
      : noCurrent && file === null
        ? 'this claim cites no source text, so there is nothing to take a digest of'
        : noCurrent
          ? 'no digest was recorded for the cited text as it now stands'
          : 'no digest was recorded when this was reviewed';
  return `The two cannot be compared here — ${why}.`;
}

// --- laws -----------------------------------------------------------------

/** A composed path, stated the way you would say it out loud. */
export function pathPhrase(path: string[]): string {
  if (path.length === 0) return 'staying where you are';
  if (path.length === 1) return `following ${path[0]}`;
  const head = path.slice(0, -1).join(', then ');
  return `following ${head}, then ${path[path.length - 1]}`;
}

/**
 * A path equation as a sentence. The kernel renders laws as
 * `Dependency.via.exposes = Dependency.provider`, which is exact and
 * unreadable; this says the same thing in words, and the caller keeps the
 * kernel's own rendering beside it so nothing is lost in translation.
 */
export function lawSentence(equation: Pick<SchemaEquation, 'start' | 'lhs' | 'rhs'>): string {
  return `Starting from any ${equation.start}, ${pathPhrase(equation.lhs)} must land on the same object as ${pathPhrase(equation.rhs)}.`;
}

/** A path as a chain of arrows for the symbolic reading: `Dependency → via → exposes`. */
export function pathChain(start: string, path: string[]): string {
  return [start, ...path].join(' → ');
}

/** How a violated law reads: the same sentence, then what actually happened. */
export function violationSentence(violation: {
  lhsPath: string[];
  rhsPath: string[];
  lhs: unknown | null;
  rhs: unknown | null;
}): string {
  if (violation.lhs === null && violation.rhs === null) {
    return 'Neither path arrives anywhere — the arrows they compose are missing.';
  }
  const missing = violation.lhs === null ? violation.lhsPath : violation.rhsPath;
  if (violation.lhs === null || violation.rhs === null) {
    return `${capitalize(pathPhrase(missing))} arrives nowhere: an arrow on that side is missing.`;
  }
  return 'Both paths arrive, but at different objects.';
}

function capitalize(text: string): string {
  return text.length === 0 ? text : text[0]!.toUpperCase() + text.slice(1);
}

// --- projections ----------------------------------------------------------

/** The one-line reading of what a projection currently contains. */
export function projectionSummary(counts: {
  objects: number;
  morphisms: number;
  truncated: boolean;
}): string {
  const objects = plural(counts.objects, 'object');
  const morphisms = plural(counts.morphisms, 'relation');
  return counts.truncated ? `first ${objects}, ${morphisms}` : `${objects}, ${morphisms}`;
}

/**
 * What a schema's registration declares, counted. Deliberately says "claims"
 * and not "claims awaiting review": the proposal store's count covers claims in
 * ANY review state, and describing them all as pending would misreport the
 * work left to do.
 */
export function registrationSummary(schema: {
  entities: readonly unknown[];
  attributeCount: number;
  arrowCount: number;
  equationCount: number;
  approvalRequired: boolean;
}): string {
  return [
    plural(schema.entities.length, 'entity', 'entities'),
    plural(schema.attributeCount, 'attribute'),
    plural(schema.arrowCount, 'arrow'),
    plural(schema.equationCount, 'law'),
    schema.approvalRequired ? 'approval required' : 'no approval required',
  ].join(' · ');
}

/**
 * Why a projected edge shows no provenance. The projection read returns
 * `(arrow, source, target)` and nothing else, so there is no citation to
 * offer — and offering a dead affordance would be worse than saying this.
 */
export const MORPHISM_PROVENANCE_NOTE =
  'Projected relations carry no citation — storage records only the arrow and its two endpoints. Select the passage in the reader to see where a relation came from.';

// --- ranking --------------------------------------------------------------

/** A rank, or the honest statement that this arm did not match. */
export function rankLabel(rank: number | null, arm: string): string {
  return rank === null ? `${arm} did not match` : `${arm} #${rank}`;
}

/** A score to four places, or the honest absence. */
export function scoreLabel(score: number | null): string {
  return score === null ? 'not recorded' : score.toFixed(4);
}

/** A boost's contribution, or the honest statement that it never fired. */
export function boostLabel(contribution: number | null): string {
  return contribution === null ? 'did not fire' : contribution.toFixed(4);
}

/** Which arms produced a hit, stated plainly. */
export function armsSentence(explain: { lexRank: number | null; semRank: number | null }): string {
  if (explain.lexRank !== null && explain.semRank !== null) {
    return 'Both arms matched, and their ranks were fused.';
  }
  if (explain.lexRank !== null) return 'Only the lexical arm matched this chunk.';
  if (explain.semRank !== null) return 'Only the semantic arm matched this chunk.';
  return 'Neither arm recorded a rank for this chunk.';
}

/** The index the semantic arm searched, or the honest statement that it did not run. */
export function indexLabel(explain: { indexVersion: string | null; embeddingModel: string | null }): string {
  if (explain.indexVersion === null) return 'the semantic arm did not run';
  return explain.embeddingModel === null
    ? explain.indexVersion
    : `${explain.indexVersion} · ${explain.embeddingModel}`;
}
