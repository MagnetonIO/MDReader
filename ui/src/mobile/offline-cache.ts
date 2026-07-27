import { load } from '@tauri-apps/plugin-store';

const CACHE_KEY = 'mobile-offline-documents';
const CACHE_VERSION = 1;
export const MAX_OFFLINE_DOCUMENTS = 64;
export const MAX_OFFLINE_DOCUMENT_BYTES = 2 * 1024 * 1024;
export const MAX_OFFLINE_CACHE_BYTES = 8 * 1024 * 1024;

export type MobileSourceIdentity =
  | { kind: 'device-folder'; id: string }
  | { kind: 'google-drive'; id: string }
  | { kind: 'dropbox'; id: string }
  | { kind: 'enterprise-catdb'; id: string };

export interface OfflineDocumentEntry {
  source: MobileSourceIdentity;
  sourceRoot: string;
  documentPath: string;
  revision: string;
  observedAt: number;
  cachedAt: number;
  freshUntil: number;
  expiresAt: number;
  content: string;
  bytes: number;
}

interface OfflineCacheEnvelope {
  version: number;
  entries: OfflineDocumentEntry[];
}

export type OfflineCacheRead =
  | {
      status: 'hit';
      freshness: 'fresh' | 'stale';
      entry: OfflineDocumentEntry;
    }
  | { status: 'miss' }
  | { status: 'expired'; entry: Omit<OfflineDocumentEntry, 'content'> }
  | { status: 'corrupt' }
  | { status: 'unavailable' };

let storePromise: ReturnType<typeof load> | undefined;

function cacheStore() {
  return (storePromise ??= load('mobile-offline-documents.json', { autoSave: false }));
}

function bytesOf(content: string): number {
  return new TextEncoder().encode(content).byteLength;
}

function validSource(value: unknown): value is MobileSourceIdentity {
  if (!value || typeof value !== 'object') return false;
  const candidate = value as Record<string, unknown>;
  return (
    (candidate.kind === 'device-folder' ||
      candidate.kind === 'google-drive' ||
      candidate.kind === 'dropbox' ||
      candidate.kind === 'enterprise-catdb') &&
    typeof candidate.id === 'string' &&
    candidate.id.length > 0 &&
    candidate.id.length <= 512
  );
}

function validNumber(value: unknown): value is number {
  return typeof value === 'number' && Number.isFinite(value) && value >= 0;
}

function entryOf(value: unknown): OfflineDocumentEntry | null {
  if (!value || typeof value !== 'object') return null;
  const candidate = value as Record<string, unknown>;
  if (
    !validSource(candidate.source) ||
    typeof candidate.sourceRoot !== 'string' ||
    candidate.sourceRoot.length === 0 ||
    typeof candidate.documentPath !== 'string' ||
    candidate.documentPath.length === 0 ||
    typeof candidate.revision !== 'string' ||
    candidate.revision.length === 0 ||
    !validNumber(candidate.observedAt) ||
    !validNumber(candidate.cachedAt) ||
    !validNumber(candidate.expiresAt) ||
    typeof candidate.content !== 'string' ||
    !validNumber(candidate.bytes)
  ) {
    return null;
  }
  const actualBytes = bytesOf(candidate.content);
  if (
    actualBytes !== candidate.bytes ||
    actualBytes > MAX_OFFLINE_DOCUMENT_BYTES ||
    candidate.expiresAt < candidate.cachedAt
  ) {
    return null;
  }
  const freshUntil = validNumber(candidate.freshUntil)
    ? Math.min(candidate.freshUntil, candidate.expiresAt)
    : candidate.cachedAt;
  return {
    source: candidate.source,
    sourceRoot: candidate.sourceRoot,
    documentPath: candidate.documentPath,
    revision: candidate.revision,
    observedAt: candidate.observedAt,
    cachedAt: candidate.cachedAt,
    freshUntil,
    expiresAt: candidate.expiresAt,
    content: candidate.content,
    bytes: actualBytes,
  };
}

function envelopeOf(value: unknown): OfflineCacheEnvelope | null {
  if (!value || typeof value !== 'object') return null;
  const candidate = value as Record<string, unknown>;
  if (candidate.version !== CACHE_VERSION || !Array.isArray(candidate.entries)) return null;
  const entries: OfflineDocumentEntry[] = [];
  for (const valueEntry of candidate.entries) {
    const entry = entryOf(valueEntry);
    if (!entry) return null;
    entries.push(entry);
  }
  if (
    entries.length > MAX_OFFLINE_DOCUMENTS ||
    entries.reduce((total, entry) => total + entry.bytes, 0) > MAX_OFFLINE_CACHE_BYTES
  ) {
    return null;
  }
  return { version: CACHE_VERSION, entries };
}

function withoutContent(entry: OfflineDocumentEntry): Omit<OfflineDocumentEntry, 'content'> {
  return {
    source: entry.source,
    sourceRoot: entry.sourceRoot,
    documentPath: entry.documentPath,
    revision: entry.revision,
    observedAt: entry.observedAt,
    cachedAt: entry.cachedAt,
    freshUntil: entry.freshUntil,
    expiresAt: entry.expiresAt,
    bytes: entry.bytes,
  };
}

export async function readOfflineDocument(
  sourceRoot: string | null,
  documentPath: string,
  now = Date.now(),
): Promise<OfflineCacheRead> {
  if (!sourceRoot) return { status: 'miss' };
  try {
    const store = await cacheStore();
    const raw = await store.get(CACHE_KEY);
    if (raw === null || raw === undefined) return { status: 'miss' };
    const envelope = envelopeOf(raw);
    if (!envelope) return { status: 'corrupt' };
    const entry = envelope.entries.find(
      (candidate) =>
        candidate.sourceRoot === sourceRoot && candidate.documentPath === documentPath,
    );
    if (!entry) return { status: 'miss' };
    if (now > entry.expiresAt) return { status: 'expired', entry: withoutContent(entry) };
    return {
      status: 'hit',
      freshness: now > entry.freshUntil ? 'stale' : 'fresh',
      entry,
    };
  } catch {
    return { status: 'unavailable' };
  }
}

export async function writeOfflineDocument(entry: OfflineDocumentEntry): Promise<boolean> {
  const normalized = entryOf(entry);
  if (!normalized) return false;
  try {
    const store = await cacheStore();
    const raw = await store.get(CACHE_KEY);
    const current = envelopeOf(raw) ?? { version: CACHE_VERSION, entries: [] };
    const deduped = current.entries.filter(
      (candidate) =>
        !(
          candidate.source.id === normalized.source.id &&
          candidate.documentPath === normalized.documentPath
        ),
    );
    deduped.push(normalized);
    deduped.sort((left, right) => right.cachedAt - left.cachedAt);
    const bounded: OfflineDocumentEntry[] = [];
    let totalBytes = 0;
    for (const candidate of deduped) {
      if (bounded.length >= MAX_OFFLINE_DOCUMENTS) break;
      if (totalBytes + candidate.bytes > MAX_OFFLINE_CACHE_BYTES) continue;
      bounded.push(candidate);
      totalBytes += candidate.bytes;
    }
    await store.set(CACHE_KEY, { version: CACHE_VERSION, entries: bounded });
    await store.save();
    return true;
  } catch {
    return false;
  }
}
