import type { components } from './openapi.gen.js';

/**
 * The server's structured error body (`ApiErrorBody` in the OpenAPI spec):
 * a stable machine-readable `error` code, a human-readable `message`, and an
 * optional `details` blob (schema-derived valid-field listings, near-miss
 * candidates, etc.). Typed straight from the generated schema — never
 * hand-authored.
 */
export type ApiErrorBody = components['schemas']['ApiErrorBody'];
/** Typed source-execution evidence attached to `entity_execution_failed`. */
export type EntityExecutionErrorDetails =
  components['schemas']['EntityExecutionErrorDetails'];

const SOURCE_OUTCOMES = new Set([
  'ran',
  'refused',
  'stale',
  'runtime_refused',
  'budget_exceeded',
  'authentication_failed',
  'rate_limited',
  'incomplete_search',
  'transport_failed',
  'source_cancelled',
  'schema_drift',
  'source_failed',
  'protocol_failed',
  'timed_out',
  'source_timed_out',
  'cancelled',
]);

function record(value: unknown): Record<string, unknown> | undefined {
  return typeof value === 'object' && value !== null
    ? (value as Record<string, unknown>)
    : undefined;
}

function nonNegativeInteger(value: unknown): boolean {
  return Number.isSafeInteger(value) && typeof value === 'number' && value >= 0;
}

/** Decode the closed execution-failure detail contract without trusting JSON. */
export function entityExecutionErrorDetails(
  value: unknown,
): EntityExecutionErrorDetails | undefined {
  const details = record(value);
  if (!details || !Array.isArray(details['sources']) || details['sources'].length === 0) {
    return undefined;
  }
  const sourceVersions = new Set<string>();
  for (const value of details['sources']) {
    const source = record(value);
    if (
      !source ||
      typeof source['source_id'] !== 'string' ||
      source['source_id'].length === 0 ||
      typeof source['source_version_id'] !== 'string' ||
      source['source_version_id'].length === 0 ||
      sourceVersions.has(source['source_version_id']) ||
      typeof source['outcome'] !== 'string' ||
      !SOURCE_OUTCOMES.has(source['outcome']) ||
      !nonNegativeInteger(source['scanned']) ||
      !nonNegativeInteger(source['returned']) ||
      (source['freshness_ms'] !== null &&
        source['freshness_ms'] !== undefined &&
        !nonNegativeInteger(source['freshness_ms'])) ||
      (source['retry_after_ms'] !== null &&
        source['retry_after_ms'] !== undefined &&
        !nonNegativeInteger(source['retry_after_ms'])) ||
      (source['outcome'] !== 'rate_limited' &&
        source['retry_after_ms'] !== null &&
        source['retry_after_ms'] !== undefined)
    ) {
      return undefined;
    }
    sourceVersions.add(source['source_version_id']);
  }
  return details as EntityExecutionErrorDetails;
}

/** Narrow an unknown parsed JSON value to {@link ApiErrorBody}. */
export function isApiErrorBody(value: unknown): value is ApiErrorBody {
  if (typeof value !== 'object' || value === null) return false;
  const body = value as Record<string, unknown>;
  return typeof body['error'] === 'string' && typeof body['message'] === 'string';
}

/** Fields carried alongside a {@link CatdbApiError}. */
export interface CatdbApiErrorInit {
  /** The HTTP status code of the failing response. */
  status: number;
  /** The stable machine-readable code (`ApiErrorBody.error`), when present. */
  code?: string;
  /** The structured `details` blob from the error body, when present. */
  details?: unknown;
  /** Typed per-source outcomes for `entity_execution_failed`. */
  execution?: EntityExecutionErrorDetails;
  /** The full parsed error body, when the response was a well-formed one. */
  body?: ApiErrorBody;
  /** The raw response text, retained when it was not a well-formed error body. */
  raw?: string;
}

/**
 * A typed error thrown for any non-2xx response from catdb-server. It maps the
 * server's {@link ApiErrorBody} onto first-class fields so callers can branch
 * on `err.code` / `err.status` instead of re-parsing a response — 4xx/5xx are
 * typed, never silent.
 */
export class CatdbApiError extends Error {
  /** The HTTP status code of the failing response. */
  readonly status: number;
  /** The stable machine-readable error code (`ApiErrorBody.error`), if any. */
  readonly code: string | undefined;
  /** The structured `details` blob, if any. */
  readonly details: unknown;
  /** Typed per-source outcomes for `entity_execution_failed`, when valid. */
  readonly execution: EntityExecutionErrorDetails | undefined;
  /** The full parsed {@link ApiErrorBody}, if the body was well-formed. */
  readonly body: ApiErrorBody | undefined;
  /** The raw response text, kept when the body was not a well-formed error. */
  readonly raw: string | undefined;

  constructor(message: string, init: CatdbApiErrorInit) {
    super(message);
    this.name = 'CatdbApiError';
    this.status = init.status;
    this.code = init.code;
    this.details = init.details;
    this.execution = init.execution;
    this.body = init.body;
    this.raw = init.raw;
    // Restore the prototype chain for `instanceof` under ES5-style down-leveling.
    Object.setPrototypeOf(this, CatdbApiError.prototype);
  }

  /**
   * Build a {@link CatdbApiError} from a failing {@link Response}, parsing the
   * body as an {@link ApiErrorBody} when possible and falling back to the raw
   * text (or a status-only message) otherwise.
   */
  static async fromResponse(response: Response): Promise<CatdbApiError> {
    let body: ApiErrorBody | undefined;
    let raw: string | undefined;
    try {
      raw = await response.text();
      if (raw.length > 0) {
        const parsed: unknown = JSON.parse(raw);
        if (isApiErrorBody(parsed)) {
          body = parsed;
          raw = undefined;
        }
      }
    } catch {
      // A non-JSON / unreadable error body: keep `raw` as-is.
    }
    const message =
      body?.message ??
      (raw && raw.length > 0
        ? `CatDB request failed (HTTP ${response.status}): ${raw}`
        : `CatDB request failed (HTTP ${response.status})`);
    return new CatdbApiError(message, {
      status: response.status,
      code: body?.error,
      details: body?.details,
      execution:
        body?.error === 'entity_execution_failed'
          ? entityExecutionErrorDetails(body.details)
          : undefined,
      body,
      raw,
    });
  }
}
