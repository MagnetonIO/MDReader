import type { PublicationEntry } from './release-pipeline';

export interface R2ObjectState {
  etag: string;
  body: Uint8Array;
}

export interface ConditionalReleaseStore {
  read(key: string): Promise<R2ObjectState | null>;
  put(
    entry: PublicationEntry,
    body: Uint8Array,
    expectedEtag?: string | null,
  ): Promise<void>;
}

function parsedStableSemver(version: string): [bigint, bigint, bigint] {
  const match =
    /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)$/.exec(version);
  if (!match) {
    throw new Error(`invalid stable-channel semantic version ${version}`);
  }
  return [BigInt(match[1]!), BigInt(match[2]!), BigInt(match[3]!)];
}

function compareStableSemver(left: string, right: string): number {
  const a = parsedStableSemver(left);
  const b = parsedStableSemver(right);
  for (let index = 0; index < 3; index += 1) {
    if (a[index] !== b[index]) return a[index]! < b[index]! ? -1 : 1;
  }
  return 0;
}

export function assertForwardStableVersion(
  currentVersion: string | null,
  nextVersion: string,
): void {
  parsedStableSemver(nextVersion);
  if (
    currentVersion !== null &&
    compareStableSemver(nextVersion, currentVersion) <= 0
  ) {
    throw new Error(
      `stable release must move forward: current ${currentVersion}, requested ${nextVersion}`,
    );
  }
}

function bytesEqual(left: Uint8Array, right: Uint8Array): boolean {
  return Buffer.from(left).equals(Buffer.from(right));
}

function pointerVersion(entry: PublicationEntry, body: Uint8Array): string {
  let parsed: { version?: unknown };
  try {
    parsed = JSON.parse(Buffer.from(body).toString('utf8')) as {
      version?: unknown;
    };
  } catch {
    throw new Error(`mutable pointer ${entry.key} is not canonical JSON`);
  }
  if (typeof parsed.version !== 'string') {
    throw new Error(`mutable pointer ${entry.key} has no semantic version`);
  }
  parsedStableSemver(parsed.version);
  return parsed.version;
}

/**
 * Publish a verified plan as a crash-resumable transaction over immutable
 * create-only objects and ETag-guarded stable pointers.
 */
export async function publishReleaseEntries(
  entries: PublicationEntry[],
  loadBody: (entry: PublicationEntry) => Promise<Uint8Array>,
  store: ConditionalReleaseStore,
): Promise<void> {
  const desired = new Map<string, Uint8Array>();
  const expectedEtags = new Map<string, string | null>();
  const skip = new Set<string>();

  for (const entry of entries) {
    const body = await loadBody(entry);
    desired.set(entry.key, body);
    const state = await store.read(entry.key);

    if (entry.immutable) {
      if (state && !bytesEqual(state.body, body)) {
        throw new Error(
          `immutable release object ${entry.key} already exists with different bytes`,
        );
      }
      if (state) skip.add(entry.key);
      continue;
    }

    const nextVersion = pointerVersion(entry, body);
    if (!state) {
      expectedEtags.set(entry.key, null);
      continue;
    }
    const currentVersion = pointerVersion(entry, state.body);
    const order = compareStableSemver(nextVersion, currentVersion);
    if (order < 0) {
      throw new Error(
        `stable release must move forward: current ${currentVersion}, requested ${nextVersion}`,
      );
    }
    if (order === 0) {
      if (!bytesEqual(state.body, body)) {
        throw new Error(
          `same-version pointer ${entry.key} does not match the exact canonical release body`,
        );
      }
      skip.add(entry.key);
      continue;
    }
    expectedEtags.set(entry.key, state.etag);
  }

  for (const entry of entries) {
    if (skip.has(entry.key)) continue;
    await store.put(
      entry,
      desired.get(entry.key)!,
      entry.immutable ? undefined : expectedEtags.get(entry.key),
    );
  }
}
