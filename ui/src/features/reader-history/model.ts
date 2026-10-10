/** Persisted receipts are history, never filesystem grants or edit authority. */
export const HISTORY_LIMITS = { tabs: 32, entries: 32, recent: 100 } as const;
export type ReaderReference =
  | { kind: 'local'; sourceId: string; path: string; revision: string }
  | { kind: 'provider'; sourceId: string; path: string; revision: string }
  | { kind: 'device'; documentId: string; sourceId: string; revision: string };
export interface ReaderPosition { revision: string; line: number; block: number; offset: number }
export interface ReaderEntry { reference: ReaderReference; title: string; heading: string | null; position: ReaderPosition | null }
export interface ReaderTab { id: string; entries: ReaderEntry[]; index: number; pinned: boolean }
export interface ReaderHistory {
  version: 1; activeTabId: string | null; tabs: ReaderTab[]; recent: ReaderEntry[]; mru: string[];
}
export type HistoryMode = 'native_atomic' | 'transient' | 'readonly';
export interface RestoredHistory { mode: HistoryMode; data: ReaderHistory; diagnostic: string | null }
export const emptyHistory = (): ReaderHistory => ({ version: 1, activeTabId: null, tabs: [], recent: [], mru: [] });
export const referenceKey = (reference: ReaderReference): string => JSON.stringify(reference.kind === 'device'
  ? [reference.kind, reference.documentId, reference.sourceId, reference.revision]
  : [reference.kind, reference.sourceId, reference.path, reference.revision]);
export const sameReference = (left: ReaderReference, right: ReaderReference): boolean => referenceKey(left) === referenceKey(right);

/** Document identity for compact Recents; exact receipts still own revision-bound state. */
export const documentIdentity = (reference: ReaderReference): string => JSON.stringify(reference.kind === 'device'
  ? [reference.kind, reference.sourceId, reference.documentId]
  : [reference.kind, reference.sourceId, reference.path]);
export const sameDocument = (left: ReaderReference, right: ReaderReference): boolean => documentIdentity(left) === documentIdentity(right);
export function recentDocuments(entries: readonly ReaderEntry[]): ReaderEntry[] {
  const seen = new Set<string>();
  // Stored MRU order, never lexical comparison of opaque revision hashes.
  return entries.filter(entry => {
    const key = documentIdentity(entry.reference);
    if (seen.has(key)) return false;
    seen.add(key);
    return true;
  });
}
