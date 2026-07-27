/**
 * The semantic command surface — CatDB's typed reading of the corpus, wrapped
 * for the showcase panels the way {@link ./fs} wraps the document surface.
 *
 * Two rules govern every normalisation here, and they are the whole point of
 * the surfaces above:
 *
 *  1. **A `null` stays `null`.** The search adapter in `./fs` coerces absent
 *     numbers to `0` because the palette only needs an ordering key. These
 *     panels report on the engine's *state*, so an unassigned schema version, a
 *     boost that never fired, and a semantic arm that never ran are all
 *     rendered as the absences they are — never as a fabricated zero.
 *  2. **Nothing is invented.** A projected morphism carries no id and no
 *     provenance because `morphisms_with_arrow_prefix` returns only
 *     `(arrow, source, target)`; the shape below says exactly that, so no
 *     component can render an affordance it cannot fill.
 *
 * Commands are reached through the generated bindings, whose `{ status }` union
 * is unwrapped into plain promises that reject with the backend's message.
 */

import {
  commands,
  type ConstraintReport as ConstraintReportDto,
  type ExplainDto,
  type FactProvenanceDto,
  type FederatedDocumentResponse as FederatedDocumentResponseDto,
  type InspectedFact as InspectedFactDto,
  type Projection as ProjectionDto,
  type SchemaDetail as SchemaDetailDto,
  type SchemaSummary as SchemaSummaryDto,
  type SearchHitDto,
  type SourceSpanDto,
  type ViolatingObjectDto,
  type ViolationDto,
} from '../bindings';

// --------------------------------------------------------------------------
// domain shapes
// --------------------------------------------------------------------------

/** One registered schema, as the Semantic view picker lists it. */
export interface SchemaSummary {
  name: string;
  /**
   * The catalog's version ordinal, or `null` when no index has been built yet.
   * `null` is NOT zero: the schema's shape is known from the domain
   * registration long before the engine assigns it a version.
   */
  version: number | null;
  /** `blake3:` checksum of the persisted schema; `null` under the same condition. */
  checksum: string | null;
  entities: string[];
  arrowCount: number;
  attributeCount: number;
  equationCount: number;
  /** From the domain registration — available even with no index. */
  approvalRequired: boolean;
  /** Objects committed to the graph. For an overlay, the approved ones. */
  objectCount: number;
  /** Claims the proposal store holds, in any review state. */
  proposalCount: number;
}

export interface FederatedProvider {
  source: string;
  attribute: string;
  verdict: string;
  value: string | null;
  reason: string | null;
  declaredFallback: string[];
}

export interface FederatedField {
  name: string;
  value: string | null;
  winner: string | null;
  providers: FederatedProvider[];
  explanation: string;
}

export interface FederatedSource {
  source: string;
  outcome: string;
  execution: string | null;
  freshness: string | null;
  ageMs: number | null;
  scanned: number;
  returned: number;
  pushed: string[];
  residual: string[];
  refusal: string | null;
}

export interface FederatedDocument {
  queryId: string;
  schema: SchemaSummary;
  completeness: string;
  document: {
    path: string;
    fields: FederatedField[];
  };
  sources: FederatedSource[];
}

/** A fact's derivation record, field for field. */
export interface FactProvenance {
  file: string | null;
  startLine: number | null;
  endLine: number | null;
  contentHash: string | null;
  schemaVersion: string | null;
  extractor: string | null;
  confidence: number | null;
  approvedBy: string | null;
}

/** One object in a projection of the corpus through a schema. */
export interface ProjectedObject {
  id: string;
  entity: string;
  /** The payload as compact JSON (specta has no `Value` type). */
  value: string;
  provenance: FactProvenance;
}

/**
 * One typed edge in a projection — arrow plus endpoint ids and NOTHING else.
 * There is deliberately no `id` and no `provenance` field: storage does not
 * know them, and `inspect_span` is the only surface that does.
 */
export interface ProjectedMorphism {
  arrow: string;
  source: string;
  target: string;
}

/** The corpus seen through one schema. */
export interface Projection {
  schema: string;
  objects: ProjectedObject[];
  morphisms: ProjectedMorphism[];
  /** True when `limit` cut the object list short. */
  truncated: boolean;
}

export interface SchemaAttribute {
  name: string;
  ty: string;
  nullable: boolean;
}

export interface SchemaEntity {
  name: string;
  attributes: SchemaAttribute[];
}

export interface SchemaArrow {
  name: string;
  dom: string;
  cod: string;
}

/** One path equation the schema imposes. */
export interface SchemaEquation {
  /** The law verbatim, as the kernel renders it. */
  law: string;
  /** The entity both sides start at. */
  start: string;
  /** Arrows composed on the left, in application order. */
  lhs: string[];
  /** Arrows composed on the right, in application order. */
  rhs: string[];
}

/** A registered schema, fully introspected. */
export interface SchemaDetail {
  name: string;
  version: number | null;
  checksum: string | null;
  typeside: string[];
  entities: SchemaEntity[];
  arrows: SchemaArrow[];
  equations: SchemaEquation[];
  approvalRequired: boolean;
}

/** A document span to inspect (absolute path, inclusive 1-based lines). */
export interface SpanSelection {
  path: string;
  startLine: number;
  endLine: number;
}

/** One extracted fact derived from a span — a *proposed claim*, not a fact. */
export interface InspectedFact {
  factId: string;
  /** `"object"` or `"morphism"`. */
  kind: string;
  schema: string;
  /** Qualified entity name (objects) or arrow name (morphisms). */
  name: string;
  /** The object's payload as compact JSON; `null` for a morphism. */
  value: string | null;
  sourceId: string | null;
  targetId: string | null;
  /** The rule that fired (e.g. `D1`) — why the extractor proposed this. */
  rule: string;
  extractor: string;
  provenance: FactProvenance;
  /** `"proposed"`, `"approved"`, or `"rejected"`. */
  state: string;
  reviewer: string | null;
  /**
   * Whether the claim is committed to the graph *right now* — read from
   * storage, never inferred from {@link state}. There is no overlay GC, so
   * `state: "proposed"` with `inGraph: true` is a legitimate, truthful pairing.
   */
  inGraph: boolean;
  /** blake3 of the cited text as it stood when the reviewer ruled. */
  reviewedContentHash: string | null;
  /** The cited text has changed since the review — re-review is a deliberate act. */
  sourceChangedSinceReview: boolean;
}

export interface ApprovalOutcome {
  factId: string;
  /** Ids newly asserted into the graph; empty for an edge still awaiting an endpoint. */
  promoted: string[];
}

export interface RejectionOutcome {
  factId: string;
  /** Ids retracted from the graph; empty when nothing had been committed. */
  retracted: string[];
  /** Morphisms rejected alongside an object rejection. */
  cascaded: string[];
}

/** A source span exactly as the engine located it. */
export interface SourceSpan {
  /** Repo-relative file. */
  file: string;
  startLine: number | null;
  endLine: number | null;
  contentHash: string | null;
}

/** An object named by a violation. */
export interface ViolatingObject {
  id: string;
  entity: string;
  /** The markdown it was derived from, when it carries a span. */
  source: SourceSpan | null;
}

/** One violated law, located in the markdown. */
export interface Violation {
  schema: string;
  law: string;
  lhsPath: string[];
  rhsPath: string[];
  subject: ViolatingObject;
  /** Where the left path lands; `null` when a morphism there is missing. */
  lhs: ViolatingObject | null;
  rhs: ViolatingObject | null;
}

export interface ConstraintReport {
  schemasChecked: number;
  violations: Violation[];
}

/** The chunk → section → document → file → source provenance chain. */
export interface HitProvenance {
  chunkId: string;
  sectionId: string;
  documentId: string;
  filePath: string;
  sourceId: string | null;
  sourceRoot: string | null;
}

/** Each boost's recorded contribution; `null` where the boost never fired. */
export interface HitBoosts {
  title: number | null;
  path: number | null;
  recency: number | null;
  graph: number | null;
}

/** The full per-hit score breakdown. */
export interface HitExplain {
  /** 1-based lexical rank, or `null` when the lexical arm did not match. */
  lexRank: number | null;
  /** 1-based semantic rank, or `null` when the semantic arm did not match. */
  semRank: number | null;
  rrfScore: number | null;
  boosts: HitBoosts;
  finalScore: number | null;
  /** The index version the semantic arm searched, or `null` when it did not run. */
  indexVersion: string | null;
  embeddingModel: string | null;
  provenance: HitProvenance;
}

/** One explained search hit. */
export interface ExplainedHit {
  chunkId: string;
  documentId: string;
  filePath: string;
  absPath: string;
  name: string;
  /** Heading breadcrumb of the chunk's section. */
  headingPath: string[];
  startLine: number;
  endLine: number;
  snippet: string;
  score: number | null;
  explain: HitExplain | null;
}

// --------------------------------------------------------------------------
// normalisation
// --------------------------------------------------------------------------

function normalizeProvenance(dto: FactProvenanceDto): FactProvenance {
  return {
    file: dto.file,
    startLine: dto.start_line,
    endLine: dto.end_line,
    contentHash: dto.content_hash,
    schemaVersion: dto.schema_version,
    extractor: dto.extractor,
    confidence: dto.confidence,
    approvedBy: dto.approved_by,
  };
}

function normalizeSummary(dto: SchemaSummaryDto): SchemaSummary {
  return {
    name: dto.name,
    version: dto.version,
    checksum: dto.checksum,
    entities: dto.entities,
    arrowCount: dto.arrow_count,
    attributeCount: dto.attribute_count,
    equationCount: dto.equation_count,
    approvalRequired: dto.approval_required,
    objectCount: dto.object_count,
    proposalCount: dto.proposal_count,
  };
}

function normalizeProjection(dto: ProjectionDto): Projection {
  return {
    schema: dto.schema,
    objects: dto.objects.map((object) => ({
      id: object.id,
      entity: object.entity,
      value: object.value,
      provenance: normalizeProvenance(object.provenance),
    })),
    morphisms: dto.morphisms.map((edge) => ({
      arrow: edge.arrow,
      source: edge.source,
      target: edge.target,
    })),
    truncated: dto.truncated,
  };
}

function normalizeDetail(dto: SchemaDetailDto): SchemaDetail {
  return {
    name: dto.name,
    version: dto.version,
    checksum: dto.checksum,
    typeside: dto.typeside,
    entities: dto.entities.map((entity) => ({
      name: entity.name,
      attributes: entity.attributes.map((attribute) => ({
        name: attribute.name,
        ty: attribute.ty,
        nullable: attribute.nullable,
      })),
    })),
    arrows: dto.arrows.map((arrow) => ({ name: arrow.name, dom: arrow.dom, cod: arrow.cod })),
    equations: dto.equations.map((equation) => ({
      law: equation.law,
      start: equation.start,
      lhs: equation.lhs,
      rhs: equation.rhs,
    })),
    approvalRequired: dto.approval_required,
  };
}

function normalizeFact(dto: InspectedFactDto): InspectedFact {
  return {
    factId: dto.fact_id,
    kind: dto.kind,
    schema: dto.schema,
    name: dto.name,
    value: dto.value,
    sourceId: dto.source_id,
    targetId: dto.target_id,
    rule: dto.rule,
    extractor: dto.extractor,
    provenance: normalizeProvenance(dto.provenance),
    state: dto.state,
    reviewer: dto.reviewer,
    inGraph: dto.in_graph,
    reviewedContentHash: dto.reviewed_content_hash,
    sourceChangedSinceReview: dto.source_changed_since_review,
  };
}

function normalizeSpan(dto: SourceSpanDto | null): SourceSpan | null {
  if (!dto) return null;
  return {
    file: dto.file,
    startLine: dto.start_line,
    endLine: dto.end_line,
    contentHash: dto.content_hash,
  };
}

function normalizeViolatingObject(dto: ViolatingObjectDto | null): ViolatingObject | null {
  if (!dto) return null;
  return { id: dto.id, entity: dto.entity, source: normalizeSpan(dto.source) };
}

function normalizeViolation(dto: ViolationDto): Violation {
  return {
    schema: dto.schema,
    law: dto.law,
    lhsPath: dto.lhs_path,
    rhsPath: dto.rhs_path,
    // The subject always exists; the two landing objects may not.
    subject: normalizeViolatingObject(dto.subject)!,
    lhs: normalizeViolatingObject(dto.lhs),
    rhs: normalizeViolatingObject(dto.rhs),
  };
}

function normalizeReport(dto: ConstraintReportDto): ConstraintReport {
  return {
    schemasChecked: dto.schemas_checked,
    violations: dto.violations.map(normalizeViolation),
  };
}

function normalizeExplain(dto: ExplainDto): HitExplain {
  return {
    lexRank: dto.lex_rank,
    semRank: dto.sem_rank,
    rrfScore: dto.rrf_score,
    boosts: {
      title: dto.boosts.title,
      path: dto.boosts.path,
      recency: dto.boosts.recency,
      graph: dto.boosts.graph,
    },
    finalScore: dto.final_score,
    indexVersion: dto.index_version,
    embeddingModel: dto.embedding_model,
    provenance: {
      chunkId: dto.provenance.chunk_id,
      sectionId: dto.provenance.section_id,
      documentId: dto.provenance.document_id,
      filePath: dto.provenance.file_path,
      sourceId: dto.provenance.source_id,
      sourceRoot: dto.provenance.source_root,
    },
  };
}

function normalizeExplainedHit(dto: SearchHitDto): ExplainedHit {
  return {
    chunkId: dto.chunk_id,
    documentId: dto.document_id,
    filePath: dto.file_path,
    absPath: dto.abs_path,
    name: dto.name,
    headingPath: dto.heading_path,
    startLine: dto.start_line,
    endLine: dto.end_line,
    snippet: dto.snippet,
    score: dto.score,
    explain: dto.explain ? normalizeExplain(dto.explain) : null,
  };
}

// --------------------------------------------------------------------------
// the adapter
// --------------------------------------------------------------------------

/** Default caps for the corpus reads (a panel, not an export). */
export const PROJECTION_LIMIT = 200;
export const EXPLAIN_LIMIT = 10;

/**
 * The transport the showcase panels consume. Every panel takes one as a prop,
 * so a test drives them with a plain object and never touches Tauri IPC.
 */
export interface SemanticsAdapter {
  listSchemas(): Promise<SchemaSummary[]>;
  projectCorpus(schema: string, limit?: number): Promise<Projection>;
  describeSchema(schema: string): Promise<SchemaDetail>;
  inspectSpan(selection: SpanSelection): Promise<InspectedFact[]>;
  /**
   * Accept a proposed claim. `reviewer` must be a real, non-blank name — the
   * backend refuses whitespace, and no caller may substitute a placeholder.
   */
  approveFact(factId: string, reviewer: string): Promise<ApprovalOutcome>;
  rejectFact(factId: string, reviewer: string): Promise<RejectionOutcome>;
  checkConstraints(): Promise<ConstraintReport>;
  explainQuery(query: string, limit?: number): Promise<ExplainedHit[]>;
  queryFederatedDocument(path: string): Promise<FederatedDocument>;
}

function unwrap<T>(result: { status: 'ok'; data: T } | { status: 'error'; error: string }): T {
  if (result.status === 'error') throw new Error(result.error);
  return result.data;
}

function normalizeFederatedDocument(dto: FederatedDocumentResponseDto): FederatedDocument {
  return {
    queryId: dto.query_id,
    schema: normalizeSummary(dto.schema),
    completeness: dto.completeness,
    document: {
      path: dto.document.path,
      fields: dto.document.fields.map((field) => ({
        name: field.name,
        value: field.value,
        winner: field.winner,
        explanation: field.explanation,
        providers: field.providers.map((provider) => ({
          source: provider.source,
          attribute: provider.attribute,
          verdict: provider.verdict,
          value: provider.value,
          reason: provider.reason,
          declaredFallback: provider.declared_fallback,
        })),
      })),
    },
    sources: dto.sources.map((source) => ({
      source: source.source,
      outcome: source.outcome,
      execution: source.execution,
      freshness: source.freshness,
      ageMs: source.age_ms,
      scanned: source.scanned,
      returned: source.returned,
      pushed: source.pushed,
      residual: source.residual,
      refusal: source.refusal,
    })),
  };
}

/** The real transport, backed by the generated Tauri command surface. */
export function makeSemanticsAdapter(): SemanticsAdapter {
  return {
    async listSchemas() {
      return unwrap(await commands.listSchemas()).map(normalizeSummary);
    },
    async projectCorpus(schema, limit = PROJECTION_LIMIT) {
      return normalizeProjection(unwrap(await commands.projectCorpus(schema, limit)));
    },
    async describeSchema(schema) {
      return normalizeDetail(unwrap(await commands.describeSchema(schema)));
    },
    async inspectSpan(selection) {
      const query = {
        path: selection.path,
        start_line: selection.startLine,
        end_line: selection.endLine,
      };
      return unwrap(await commands.inspectSpan(query)).map(normalizeFact);
    },
    async approveFact(factId, reviewer) {
      const dto = unwrap(await commands.approveFact(factId, reviewer));
      return { factId: dto.fact_id, promoted: dto.promoted };
    },
    async rejectFact(factId, reviewer) {
      const dto = unwrap(await commands.rejectFact(factId, reviewer));
      return { factId: dto.fact_id, retracted: dto.retracted, cascaded: dto.cascaded };
    },
    async checkConstraints() {
      return normalizeReport(unwrap(await commands.checkConstraints()));
    },
    async explainQuery(query, limit = EXPLAIN_LIMIT) {
      return unwrap(await commands.explainQuery(query, limit)).map(normalizeExplainedHit);
    },
    async queryFederatedDocument(path) {
      return normalizeFederatedDocument(unwrap(await commands.queryFederatedDocument(path)));
    },
  };
}
