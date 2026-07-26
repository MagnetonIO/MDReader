import { createHash } from 'node:crypto';

import type {
  PublicationEntry,
  PublicationKind,
} from './release-pipeline';

const SOURCE_COMMIT = /^[0-9a-f]{40}$/;
const SHA256 = /^[0-9a-f]{64}$/;
const STABLE_VERSION =
  /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/;
const KINDS = new Set<PublicationKind>([
  'artifact',
  'sbom',
  'signature',
  'manifest',
  'updater-manifest',
  'pointer',
]);
const MAX_ENTRIES = 64;
const MAX_KEY_BYTES = 512;
const MAX_ENTRY_BYTES = 1024 * 1024 * 1024;
const MAX_TOTAL_BYTES = 4 * MAX_ENTRY_BYTES;

export interface SealedPublicationEntry {
  kind: PublicationKind;
  key: string;
  immutable: boolean;
  bytes: number;
  sha256: string;
  bodyBase64: string;
}

export interface SealedPublicationPlan {
  schemaVersion: 1;
  sourceCommit: string;
  version: string;
  entries: SealedPublicationEntry[];
}

function exactKeys(
  input: Record<string, unknown>,
  expected: readonly string[],
  path: string,
): void {
  const actual = Object.keys(input).sort();
  const wanted = [...expected].sort();
  if (
    actual.length !== wanted.length ||
    actual.some((key, index) => key !== wanted[index])
  ) {
    throw new Error(`${path} contains unknown or missing fields`);
  }
}

function record(value: unknown, path: string): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new Error(`${path} must be an object`);
  }
  return value as Record<string, unknown>;
}

function sha256(body: Uint8Array): string {
  return createHash('sha256').update(body).digest('hex');
}

function canonicalKey(
  key: unknown,
  immutable: boolean,
  version: string,
): string {
  if (
    typeof key !== 'string' ||
    key === '' ||
    Buffer.byteLength(key, 'utf8') > MAX_KEY_BYTES ||
    key.startsWith('/') ||
    key.includes('\\') ||
    key.includes('%') ||
    key.split('/').some((part) => part === '' || part === '.' || part === '..')
  ) {
    throw new Error('sealed publication entry has a noncanonical object key');
  }
  if (immutable) {
    if (!key.startsWith(`releases/v${version}/`)) {
      throw new Error('immutable publication entry is outside the release version');
    }
  } else if (
    key !== 'channels/stable/updater.json' &&
    key !== 'channels/stable/manifest.json'
  ) {
    throw new Error('mutable publication entry is not a reviewed stable pointer');
  }
  return key;
}

function pointerVersion(body: Uint8Array, key: string): string {
  let parsed: Record<string, unknown>;
  try {
    parsed = record(JSON.parse(Buffer.from(body).toString('utf8')), key);
  } catch {
    throw new Error(`sealed stable pointer ${key} is not JSON`);
  }
  if (typeof parsed.version !== 'string') {
    throw new Error(`sealed stable pointer ${key} has no version`);
  }
  return parsed.version;
}

export async function createSealedPublicationPlan(
  entries: PublicationEntry[],
  sourceCommit: string,
  version: string,
  loadBody: (entry: PublicationEntry) => Promise<Uint8Array>,
): Promise<SealedPublicationPlan> {
  if (!SOURCE_COMMIT.test(sourceCommit)) {
    throw new Error('sealed publication source commit is invalid');
  }
  if (!STABLE_VERSION.test(version)) {
    throw new Error('sealed publication version must be stable SemVer');
  }
  const sealed: SealedPublicationEntry[] = [];
  for (const entry of entries) {
    const body = await loadBody(entry);
    sealed.push({
      kind: entry.kind,
      key: canonicalKey(entry.key, entry.immutable, version),
      immutable: entry.immutable,
      bytes: body.byteLength,
      sha256: sha256(body),
      bodyBase64: Buffer.from(body).toString('base64'),
    });
  }
  return parseSealedPublicationPlan({
    schemaVersion: 1,
    sourceCommit,
    version,
    entries: sealed,
  });
}

export function parseSealedPublicationPlan(
  value: unknown,
): SealedPublicationPlan {
  const input = record(value, 'sealed publication plan');
  exactKeys(
    input,
    ['schemaVersion', 'sourceCommit', 'version', 'entries'],
    'sealed publication plan',
  );
  if (input.schemaVersion !== 1) {
    throw new Error('sealed publication plan schemaVersion must be 1');
  }
  if (typeof input.sourceCommit !== 'string' || !SOURCE_COMMIT.test(input.sourceCommit)) {
    throw new Error('sealed publication plan source commit is invalid');
  }
  if (typeof input.version !== 'string' || !STABLE_VERSION.test(input.version)) {
    throw new Error('sealed publication plan version must be stable SemVer');
  }
  if (!Array.isArray(input.entries) || input.entries.length < 3) {
    throw new Error('sealed publication plan entries are incomplete');
  }
  if (input.entries.length > MAX_ENTRIES) {
    throw new Error('sealed publication plan has too many entries to be bounded');
  }

  const seen = new Set<string>();
  let totalBytes = 0;
  const entries = input.entries.map((value, index): SealedPublicationEntry => {
    const entry = record(value, `sealed publication plan entries[${index}]`);
    exactKeys(
      entry,
      ['kind', 'key', 'immutable', 'bytes', 'sha256', 'bodyBase64'],
      `sealed publication plan entries[${index}]`,
    );
    if (typeof entry.kind !== 'string' || !KINDS.has(entry.kind as PublicationKind)) {
      throw new Error(`sealed publication entry ${index} has an invalid kind`);
    }
    if (typeof entry.immutable !== 'boolean') {
      throw new Error(`sealed publication entry ${index} has invalid immutability`);
    }
    const key = canonicalKey(entry.key, entry.immutable, input.version as string);
    if (seen.has(key)) throw new Error(`sealed publication plan duplicates ${key}`);
    seen.add(key);
    if (
      typeof entry.bodyBase64 !== 'string' ||
      entry.bodyBase64.length > Math.ceil((MAX_ENTRY_BYTES * 4) / 3) + 4 ||
      !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(
        entry.bodyBase64,
      )
    ) {
      throw new Error(`sealed publication entry ${key} has invalid body encoding`);
    }
    const body = Buffer.from(entry.bodyBase64, 'base64');
    if (
      !Number.isSafeInteger(entry.bytes) ||
      (entry.bytes as number) < 1 ||
      (entry.bytes as number) > MAX_ENTRY_BYTES ||
      body.byteLength !== entry.bytes
    ) {
      throw new Error(`sealed publication entry ${key} has invalid byte length`);
    }
    if (
      typeof entry.sha256 !== 'string' ||
      !SHA256.test(entry.sha256) ||
      sha256(body) !== entry.sha256
    ) {
      throw new Error(`sealed publication entry ${key} has invalid SHA-256`);
    }
    totalBytes += body.byteLength;
    if (!Number.isSafeInteger(totalBytes) || totalBytes > MAX_TOTAL_BYTES) {
      throw new Error('sealed publication plan exceeds its bounded byte limit');
    }
    if (!entry.immutable && pointerVersion(body, key) !== input.version) {
      throw new Error(`sealed stable pointer ${key} has a divergent version`);
    }
    return {
      kind: entry.kind as PublicationKind,
      key,
      immutable: entry.immutable,
      bytes: entry.bytes as number,
      sha256: entry.sha256,
      bodyBase64: entry.bodyBase64,
    };
  });

  const mutable = entries.filter((entry) => !entry.immutable);
  if (
    mutable.length !== 2 ||
    mutable[0]?.key !== 'channels/stable/manifest.json' ||
    mutable[1]?.key !== 'channels/stable/updater.json' ||
    entries.at(-2)?.key !== mutable[0].key ||
    entries.at(-1)?.key !== mutable[1].key
  ) {
    throw new Error('sealed publication plan must end with both stable pointers');
  }
  if (
    !entries.some(
      (entry) =>
        entry.kind === 'manifest' &&
        entry.key === `releases/v${input.version}/manifest.json`,
    )
  ) {
    throw new Error('sealed publication plan is missing its immutable manifest');
  }

  return {
    schemaVersion: 1,
    sourceCommit: input.sourceCommit,
    version: input.version,
    entries,
  };
}

export function sealedPublicationPlanSha256(
  contents: string | Uint8Array,
): string {
  return createHash('sha256').update(contents).digest('hex');
}
