import { readerBackend, unwrapNative } from '../../platform/backend';

export interface RecoveredDraft {
  draftId: string; revision: string; title: string; sourceName: string;
  state: 'absent' | 'prepared' | 'dispatched' | 'unknown' | 'accepted' | 'published' | 'conflict'; byteLength: number;
}
export interface RecoveredDraftPage { drafts: RecoveredDraft[]; offset: number; nextOffset: number | null }
export interface RecoveredDraftBody { draft: RecoveredDraft; markdown: string }
export interface RecoveryBackend {
  list(offset: number): Promise<RecoveredDraftPage>;
  read(draft: RecoveredDraft): Promise<RecoveredDraftBody>;
}
const states = new Set(['absent', 'prepared', 'dispatched', 'unknown', 'accepted', 'published', 'conflict']);
function valid(value: RecoveredDraft): boolean {
  return Boolean(value && /^[a-f0-9]{8}-[a-f0-9]{4}-4[a-f0-9]{3}-[89ab][a-f0-9]{3}-[a-f0-9]{12}$/.test(value.draftId)
    && /^[a-f0-9]{64}$/.test(value.revision) && states.has(value.state)
    && Number.isSafeInteger(value.byteLength) && value.byteLength >= 0 && value.byteLength <= 2 * 1024 * 1024
    && [value.title, value.sourceName].every(text => typeof text === 'string' && text.length > 0 && text.length <= 512 && !/[\u0000-\u001f\u007f]/.test(text)));
}
export const cloudRecoveryBackend: RecoveryBackend = {
  async list(offset) {
    const result = await unwrapNative(readerBackend().providers.nativeProviderRequest({ operation: 'recovered_drafts', offset, limit: 32 }));
    if (result.operation !== 'recovered_drafts' || result.offset !== offset || !Array.isArray(result.drafts) || result.drafts.length > 32
      || !result.drafts.every(valid) || new Set(result.drafts.map(draft => draft.draftId)).size !== result.drafts.length
      || result.nextOffset !== null && (!Number.isSafeInteger(result.nextOffset) || result.nextOffset <= offset))
      throw new Error('Retained cloud drafts could not be verified.');
    return result;
  },
  async read(draft) {
    const result = await unwrapNative(readerBackend().providers.nativeProviderRequest({ operation: 'read_recovered_draft', draftId: draft.draftId, revision: draft.revision }));
    if (result.operation !== 'read_recovered_draft' || !valid(result.draft) || result.draft.draftId !== draft.draftId || result.draft.revision !== draft.revision || typeof result.markdown !== 'string')
      throw new Error('This retained draft could not be verified. Reload the draft list.');
    const bytes = new TextEncoder().encode(result.markdown);
    if (bytes.length !== result.draft.byteLength || bytes.length > 2 * 1024 * 1024) throw new Error('This retained draft exceeds the copy limit or changed.');
    const hash = [...new Uint8Array(await crypto.subtle.digest('SHA-256', bytes))].map(byte => byte.toString(16).padStart(2, '0')).join('');
    if (hash !== draft.revision) throw new Error('This retained draft changed. Reload the draft list.');
    return result;
  },
};
