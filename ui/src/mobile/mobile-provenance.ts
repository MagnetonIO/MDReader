import type { EntityQueryResponse, EntityRow } from '@catdb/client';

const MAX_VERSION_COUNT = 1_000;
const MAX_FIELD_COUNT = 64;
const MAX_OBSERVATIONS_PER_FIELD = 64;
const MAX_TEXT = 1_024;

type SourceOutcome = EntityQueryResponse['sources'][number]['outcome'];

const SOURCE_OUTCOMES = {
  ran: true,
  refused: true,
  stale: true,
  runtime_refused: true,
  budget_exceeded: true,
  authentication_failed: true,
  rate_limited: true,
  incomplete_search: true,
  transport_failed: true,
  source_cancelled: true,
  schema_drift: true,
  source_failed: true,
  protocol_failed: true,
  timed_out: true,
  source_timed_out: true,
  cancelled: true,
} as const satisfies Record<SourceOutcome, true>;

export interface MobileDocumentProvenance {
  pins: EntityQueryResponse['pins'];
  fields: EntityRow['provenance'];
  sources: EntityQueryResponse['sources'];
}

function record(value: unknown): Record<string, unknown> | null {
  return typeof value === 'object' && value !== null
    ? (value as Record<string, unknown>)
    : null;
}

function text(value: unknown, max = MAX_TEXT): value is string {
  return typeof value === 'string' && value.length > 0 && value.length <= max;
}

function boundedStrings(value: unknown): value is string[] {
  return (
    Array.isArray(value) &&
    value.length <= MAX_VERSION_COUNT &&
    value.every((item) => text(item))
  );
}

function nonNegative(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0;
}

function sourceOutcome(value: unknown): value is SourceOutcome {
  return typeof value === 'string' && Object.hasOwn(SOURCE_OUTCOMES, value);
}

function validState(value: unknown): boolean {
  const state = record(value);
  if (!state || typeof state.state !== 'string') return false;
  switch (state.state) {
    case 'observed_value':
      return Object.hasOwn(state, 'value');
    case 'observed_null':
    case 'field_unseen':
    case 'record_unseen':
      return true;
    case 'not_asked': {
      const reason = record(state.value);
      return text(reason?.reason, 4_096);
    }
    default:
      return false;
  }
}

/** Validate and retain the complete immutable pins and per-field observations. */
export function mobileProvenanceOf(
  value: unknown,
  expectedRecordKey?: string,
  completeness?: EntityQueryResponse['completeness'],
): MobileDocumentProvenance | null {
  const candidate = record(value);
  const pins = record(candidate?.pins);
  const fields = record(candidate?.fields);
  if (
    !candidate ||
    !pins ||
    !fields ||
    !text(pins.catalog_resolver_id) ||
    !text(pins.catalog_revision_id) ||
    !text(pins.schema_version_id) ||
    !text(pins.provenance_rule_version_id) ||
    !boundedStrings(pins.physical_schema_version_ids) ||
    !boundedStrings(pins.mapping_version_ids) ||
    !boundedStrings(pins.source_version_ids) ||
    !boundedStrings(pins.policy_version_ids) ||
    !Array.isArray(candidate.sources) ||
    candidate.sources.length > MAX_VERSION_COUNT ||
    Object.keys(fields).length > MAX_FIELD_COUNT
  ) {
    return null;
  }
  const physicalSchemaVersions = pins.physical_schema_version_ids;
  const mappingVersions = pins.mapping_version_ids;
  const sourceVersions = pins.source_version_ids;
  const policyVersions = pins.policy_version_ids;
  if (
    sourceVersions.length === 0 ||
    sourceVersions.length !== candidate.sources.length ||
    mappingVersions.length !== candidate.sources.length ||
    physicalSchemaVersions.length !== candidate.sources.length ||
    policyVersions.length === 0 ||
    new Set(sourceVersions).size !== sourceVersions.length
  ) {
    return null;
  }
  for (const [sourceIndex, sourceValue] of candidate.sources.entries()) {
    const source = record(sourceValue);
    if (
      !source ||
      !text(source.source_id) ||
      !text(source.source_version_id) ||
      source.source_version_id !== sourceVersions[sourceIndex] ||
      !nonNegative(source.scanned) ||
      !nonNegative(source.returned) ||
      (source.freshness_ms !== null &&
        source.freshness_ms !== undefined &&
        !nonNegative(source.freshness_ms)) ||
      (source.retry_after_ms !== null &&
        source.retry_after_ms !== undefined &&
        (!Number.isSafeInteger(source.retry_after_ms) ||
          !nonNegative(source.retry_after_ms))) ||
      (source.outcome !== 'rate_limited' &&
        source.retry_after_ms !== null &&
        source.retry_after_ms !== undefined) ||
      !sourceOutcome(source.outcome)
    ) {
      return null;
    }
  }
  if (completeness !== undefined) {
    const state = record(completeness);
    if (!state || (state.state !== 'complete' && state.state !== 'partial')) {
      return null;
    }
    const failedSourceIds = candidate.sources.flatMap((value) => {
      const source = record(value);
      return source &&
        source.outcome !== 'ran' &&
        typeof source.source_id === 'string'
        ? [source.source_id]
        : [];
    });
    if (state.state === 'complete') {
      if (failedSourceIds.length !== 0) return null;
    } else {
      const excludedSourceIds = state.excluded_source_ids;
      if (
        !Array.isArray(excludedSourceIds) ||
        !excludedSourceIds.every((sourceId) => text(sourceId)) ||
        new Set(excludedSourceIds).size !== excludedSourceIds.length ||
        failedSourceIds.length !== excludedSourceIds.length ||
        failedSourceIds.some((sourceId) => !excludedSourceIds.includes(sourceId))
      ) {
        return null;
      }
    }
  }
  for (const [fieldName, provenanceValue] of Object.entries(fields)) {
    const provenance = record(provenanceValue);
    if (
      !text(fieldName, 4_096) ||
      provenance?.field !== fieldName ||
      provenance.provenance_rule_version_id !== pins.provenance_rule_version_id ||
      !Array.isArray(provenance.considered) ||
      provenance.considered.length !== candidate.sources.length ||
      provenance.considered.length > MAX_OBSERVATIONS_PER_FIELD
    ) {
      return null;
    }
    const selected = provenance.selected;
    if (
      selected !== null &&
      selected !== undefined &&
      (!Number.isInteger(selected) ||
        typeof selected !== 'number' ||
        selected < 0 ||
        selected >= provenance.considered.length)
    ) {
      return null;
    }
    for (const [sourceIndex, observationValue] of provenance.considered.entries()) {
      const observation = record(observationValue);
      if (
        !observation ||
        !text(observation.source_version_id) ||
        observation.source_version_id !== sourceVersions[sourceIndex] ||
        !text(observation.mapping_version_id) ||
        observation.mapping_version_id !== mappingVersions[sourceIndex] ||
        !text(observation.record_key) ||
        (expectedRecordKey !== undefined &&
          observation.record_key !== expectedRecordKey) ||
        (observation.observed_at_ms !== null &&
          observation.observed_at_ms !== undefined &&
          !nonNegative(observation.observed_at_ms)) ||
        !validState(observation.state)
      ) {
        return null;
      }
    }
  }
  return value as MobileDocumentProvenance;
}
