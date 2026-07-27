import type { EntityQueryResponse, EntityRow } from '@catdb/client';

const MAX_VERSION_COUNT = 1_000;
const MAX_FIELD_COUNT = 64;
const MAX_OBSERVATIONS_PER_FIELD = 64;
const MAX_TEXT = 1_024;

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
  const sourceVersions = new Set(pins.source_version_ids);
  const mappingVersions = new Set(pins.mapping_version_ids);
  const visibleSourceVersions = new Set<string>();
  for (const sourceValue of candidate.sources) {
    const source = record(sourceValue);
    if (
      !source ||
      !text(source.source_id) ||
      !text(source.source_version_id) ||
      !sourceVersions.has(source.source_version_id) ||
      visibleSourceVersions.has(source.source_version_id) ||
      !nonNegative(source.scanned) ||
      !nonNegative(source.returned) ||
      (source.freshness_ms !== null &&
        source.freshness_ms !== undefined &&
        !nonNegative(source.freshness_ms)) ||
      typeof source.outcome !== 'string'
    ) {
      return null;
    }
    visibleSourceVersions.add(source.source_version_id);
  }
  for (const [fieldName, provenanceValue] of Object.entries(fields)) {
    const provenance = record(provenanceValue);
    if (
      !text(fieldName, 4_096) ||
      provenance?.field !== fieldName ||
      !Array.isArray(provenance.considered) ||
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
    for (const observationValue of provenance.considered) {
      const observation = record(observationValue);
      if (
        !observation ||
        !text(observation.source_version_id) ||
        !sourceVersions.has(observation.source_version_id) ||
        !visibleSourceVersions.has(observation.source_version_id) ||
        !text(observation.mapping_version_id) ||
        !mappingVersions.has(observation.mapping_version_id) ||
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
