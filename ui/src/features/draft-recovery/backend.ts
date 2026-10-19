import { readerBackend } from '../../platform/backend';

export type DraftKey = { kind: 'local' | 'provider'; source_id: string; path: string } | { kind: 'device'; document_id: string };
export interface DraftMetadata {
  draft_id: string; key: DraftKey; generation: number; base_revision: string; base_hash: string | null;
  title: string; draft_hash: string; bytes: number; modified_ms: number;
}
export interface DraftSlot { draft_id: string; generation: number; draft: DraftMetadata | null }
export interface DraftSnapshot { key: DraftKey; base_revision: string; base_hash: string | null; title: string; markdown: string; dirty: boolean }
export type DraftRequest = { operation: 'inspect'; key: DraftKey }
  | { operation: 'checkpoint'; key: DraftKey; expected_generation: number; draft: Omit<DraftSnapshot, 'key' | 'dirty'> }
  | { operation: 'read'; draft_id: string; generation: number }
  | { operation: 'discard'; draft_id: string; expected_generation: number };
export type DraftReply = { result: 'slot'; slot: DraftSlot } | { result: 'read'; draft: DraftMetadata; markdown: string };
export type DraftBackend = (request: DraftRequest) => Promise<DraftReply>;
export function draftKeyId(key: DraftKey): string { return JSON.stringify(key.kind === 'device' ? ['device', key.document_id] : [key.kind, key.source_id, key.path]); }
export function sameBase(draft: DraftMetadata, snapshot: DraftSnapshot): boolean {
  return draftKeyId(draft.key) === draftKeyId(snapshot.key) && draft.base_revision === snapshot.base_revision
    && (draft.base_hash === null || draft.base_hash === snapshot.base_hash);
}
/** Draft locators are evidence only. This command never opens or writes an original. */
export const draftBackend: DraftBackend = async (request) => {
  if (new TextEncoder().encode(JSON.stringify(request)).length > 2 * 1024 * 1024 - 4096)
    throw new Error('This draft is too large for automatic recovery. Save a copy to keep it.');
  const response = await readerBackend().persistence.nativeEditorDrafts(JSON.stringify(request));
  if (response.status === 'error') throw new Error('Automatic draft recovery is unavailable. Save your changes or a copy to keep them.');
  const reply = JSON.parse(response.data) as DraftReply;
  const metadata = reply?.result === 'slot' ? reply.slot?.draft : reply?.result === 'read' ? reply.draft : undefined;
  const generation = reply?.result === 'slot' ? reply.slot?.generation : metadata?.generation;
  if (!Number.isSafeInteger(generation) || generation! < 0
    || reply.result !== 'slot' && reply.result !== 'read'
    || metadata && (!metadata.key || typeof metadata.base_revision !== 'string' || !/^[a-f0-9]{64}$/.test(metadata.draft_hash))
    || request.operation === 'read' && (reply.result !== 'read' || reply.draft.draft_id !== request.draft_id
      || reply.draft.generation !== request.generation || typeof reply.markdown !== 'string'))
    throw new Error('Draft recovery could not be verified. Your editor text is kept.');
  return reply;
};
