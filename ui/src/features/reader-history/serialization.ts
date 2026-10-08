import { emptyHistory, HISTORY_LIMITS, type ReaderEntry, type ReaderPosition, type ReaderReference, type RestoredHistory } from './model';
const bytes = (text: string) => new TextEncoder().encode(text).length;
const hash = /^[a-f0-9]{64}$/;
const uuid = /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/;
const object = (value: unknown): value is Record<string, unknown> => typeof value === 'object' && value !== null && !Array.isArray(value);
const integer = (value: unknown, min: number, max: number): value is number => typeof value === 'number' && Number.isSafeInteger(value) && value >= min && value <= max;
export function parseReference(value: unknown): ReaderReference | null {
  if (!object(value) || typeof value.sourceId !== 'string' || typeof value.revision !== 'string' || !hash.test(value.revision)) return null;
  if (value.kind === 'device') return hash.test(value.sourceId) && typeof value.documentId === 'string' && uuid.test(value.documentId)
    ? { kind: 'device', sourceId: value.sourceId, revision: value.revision, documentId: value.documentId } : null;
  if (value.kind !== 'local' && value.kind !== 'provider' || typeof value.path !== 'string' || !value.path || bytes(value.path) > 4096
    || value.path.includes('\\') || value.path.startsWith('/') || value.path.split('/').some((part) => !part || part === '.' || part === '..')
    || [...value.path].some((c) => c.charCodeAt(0) < 32 || c.charCodeAt(0) >= 127 && c.charCodeAt(0) <= 159)) return null;
  if (value.kind === 'local' ? !hash.test(value.sourceId)
    : !/^provider:[a-f0-9]{64}$/.test(value.sourceId) || !/^records\/[a-f0-9]{64}\.md$/.test(value.path)) return null;
  return { kind: value.kind, sourceId: value.sourceId, path: value.path, revision: value.revision };
}
export function parsePosition(value: unknown, revision: string): ReaderPosition | null {
  if (!object(value) || value.revision !== revision || !integer(value.line, 1, 0x7fff_ffff)
    || !integer(value.block, 0, 0x7fff_ffff) || typeof value.offset !== 'number' || !Number.isFinite(value.offset)
    || value.offset < 0 || value.offset > 1_000_000) return null;
  return { revision, line: value.line, block: value.block, offset: value.offset };
}
function entry(value: unknown): ReaderEntry | null {
  if (!object(value) || typeof value.title !== 'string' || bytes(value.title) > 512
    || value.heading !== null && (typeof value.heading !== 'string' || bytes(value.heading) > 512)) return null;
  const reference = parseReference(value.reference);
  if (!reference) return null;
  const position = value.position === null ? null : parsePosition(value.position, reference.revision);
  if (value.position !== null && !position) return null;
  return { reference, title: value.title, heading: value.heading as string | null, position };
}
function entries(value: unknown, max: number): ReaderEntry[] | null {
  if (!Array.isArray(value) || value.length > max) return null;
  const parsed = value.map(entry);
  return parsed.every((item): item is ReaderEntry => item !== null) ? parsed : null;
}
/** Unrecognized rows make the namespace read-only; never normalize them away. */
export function restoreReaderHistory(value: unknown, nativeAtomic: boolean): RestoredHistory {
  if (!nativeAtomic) return { mode: 'transient', data: emptyHistory(), diagnostic: 'Reading history is session-only on this host. Durable Windows parity is unfinished.' };
  if (value === undefined) return { mode: 'native_atomic', data: emptyHistory(), diagnostic: null };
  const unavailable = { mode: 'readonly' as const, data: emptyHistory(), diagnostic: 'Saved reading history is unsupported or malformed. Its original data is preserved; new reading activity will not overwrite it.' };
  if (!object(value) || value.version !== 1 || !Array.isArray(value.tabs) || value.tabs.length > HISTORY_LIMITS.tabs
    || !Array.isArray(value.mru) || value.mru.some((id) => typeof id !== 'string')) return unavailable;
  const recent = entries(value.recent, HISTORY_LIMITS.recent);
  if (!recent) return unavailable;
  const tabs = [];
  const ids = new Set<string>();
  for (const item of value.tabs) {
    if (!object(item) || typeof item.id !== 'string' || !uuid.test(item.id) || ids.has(item.id) || typeof item.pinned !== 'boolean') return unavailable;
    const history = entries(item.entries, HISTORY_LIMITS.entries);
    if (!history?.length || !integer(item.index, 0, history.length - 1)) return unavailable;
    ids.add(item.id); tabs.push({ id: item.id, entries: history, index: item.index, pinned: item.pinned });
  }
  if (value.activeTabId !== null && (typeof value.activeTabId !== 'string' || !ids.has(value.activeTabId))
    || value.mru.length !== ids.size || new Set(value.mru).size !== ids.size || value.mru.some((id) => !ids.has(id))) return unavailable;
  return { mode: 'native_atomic', data: { version: 1, activeTabId: value.activeTabId as string | null, tabs, recent, mru: value.mru as string[] }, diagnostic: null };
}
