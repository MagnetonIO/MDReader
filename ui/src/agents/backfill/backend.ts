import type { AgentBackfillAdapter, AgentBackfillReply, AgentBackfillRequest, AgentBackfillReview } from '../../bindings';
import { readerBackend } from '../../platform/backend';
export type { AgentBackfillAdapter, AgentBackfillReply, AgentBackfillRequest, AgentBackfillReview };
export interface BackfillBackend { request(root: string, request: AgentBackfillRequest): Promise<AgentBackfillReply> }
export const nativeBackfillBackend: BackfillBackend = {
  async request(root, request) {
    const result = await readerBackend().agents.agentCaptureBackfill(root, request);
    if (result.status === 'error') throw new Error(result.error);
    return result.data;
  },
};
/** Decode only the individually selected event, never paths mentioned inside it.
 * Fatal UTF-8 decoding avoids changing invalid bytes into invented visible text. */
export async function selectedEvent(file: File): Promise<string> {
  if (file.size < 1 || file.size > 256 * 1024) throw new Error('Choose one final-output JSON event of at most 256 KiB.');
  const bytes = await file.arrayBuffer();
  if (bytes.byteLength !== file.size) throw new Error('The selected event changed while reading.');
  try { return new TextDecoder('utf-8', { fatal: true }).decode(bytes); }
  catch { throw new Error('The selected event must contain valid UTF-8 JSON.'); }
}
