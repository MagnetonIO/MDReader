import { invoke } from '@tauri-apps/api/core';
import type { ArchivedReviewRecord, ArchivedReviewComparison, ArchivedReviewsReply, ReviewCurrent } from '../bindings';
export type { ArchivedReviewRecord, ArchivedReviewComparison, ReviewCurrent };
export interface ArchiveSelection { handle: string; exportId: string; manifestSha256: string; ownerScope: string }
export interface ArchivedReviewsBackend {
  select(exportId: string, manifestSha256: string): Promise<ArchiveSelection | null>;
  list(handle: string, offset: number): Promise<{ records: ArchivedReviewRecord[]; nextOffset: number | null }>;
  current(path: string, generation: number): Promise<ReviewCurrent>;
  compare(handle: string, recordId: string, current: ReviewCurrent, generation: number | null): Promise<ArchivedReviewComparison>;
  close(handle: string): Promise<void>;
}
export class ArchivedReviewsError extends Error {
  constructor(readonly kind: string) {
    super(kind === 'busy' ? 'Another archive operation is active. Try again when it finishes.'
      : kind === 'conflict' ? 'The selected archive, folder or exact document revision changed. Select it again.'
      : kind === 'unsupported' ? 'This archive or comparison is unsupported. Its original records remain preserved.'
      : kind === 'too_large' ? 'This selection exceeds the supported review bounds.'
      : kind === 'invalid' ? 'Choose an explicit export UUID, manifest hash and supported review stage.'
      : 'The selected archive or current document is unavailable. No comparison was confirmed.');
  }
}
async function request(value: Record<string, unknown>): Promise<ArchivedReviewsReply> {
  try { return await invoke<ArchivedReviewsReply>('reader_archived_reviews', { request: value }); }
  catch (cause) { throw new ArchivedReviewsError(cause && typeof cause === 'object' && 'kind' in cause && typeof cause.kind === 'string' ? cause.kind : 'unavailable'); }
}
export const archivedReviewsBackend: ArchivedReviewsBackend = {
  async select(exportId, manifestSha256) {
    const reply = await request({ operation: 'select', exportId, manifestSha256 });
    if (reply.operation === 'cancelled') return null;
    if (reply.operation !== 'selected') throw new ArchivedReviewsError('unavailable');
    return reply;
  },
  async list(handle, offset) {
    const reply = await request({ operation: 'list', handle, offset, limit: 20 });
    if (reply.operation !== 'listed') throw new ArchivedReviewsError('unavailable');
    return reply;
  },
  async current(path, generation) {
    const reply = await request({ operation: 'current', path, generation });
    if (reply.operation !== 'current') throw new ArchivedReviewsError('unavailable');
    return reply.reference;
  },
  async compare(handle, recordId, current, generation) {
    const reference = { sourceId: current.sourceId, path: current.path, revision: current.revision };
    const reply = await request({ operation: 'compare', handle, recordId, current: reference, generation });
    if (reply.operation !== 'compared') throw new ArchivedReviewsError('unavailable');
    return reply.comparison;
  },
  async close(handle) { await request({ operation: 'close', handle }); },
};
