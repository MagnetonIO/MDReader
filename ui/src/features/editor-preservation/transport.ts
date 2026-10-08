/** Output-only archival IPC. These bodies never authorize a filesystem save. */
import { invoke } from '@tauri-apps/api/core';
import type { EditorEntry } from '../../stores/editor';
import type { FlushRequest } from '../../shell/persistence';

export interface EditorSubmission {
  request: FlushRequest;
  generation: string;
  entries: Array<EditorEntry & { entryId: string }>;
  providerEditsJson: string;
}
interface EditorReceipt extends FlushRequest {
  generation: string;
  sha256: string;
  entryCount: number;
}
const LIMIT = 16 * 1024 * 1024;

/** Bound encoded JSON before allocating its full escaped representation. */
export function boundedJson(value: unknown): string {
  let bytes = 0;
  const add = (amount: number) => {
    bytes += amount;
    if (bytes > LIMIT) throw new Error('Editor preparation exceeds its archival size limit');
  };
  function string(text: string) {
    add(2);
    for (let i = 0; i < text.length; i++) {
      const c = text.charCodeAt(i);
      if (c === 34 || c === 92) add(2);
      else if (c < 32) add([8, 9, 10, 12, 13].includes(c) ? 2 : 6);
      else if (c < 128) add(1);
      else if (c < 2048) add(2);
      else if (c >= 0xd800 && c <= 0xdbff && text.charCodeAt(i + 1) >= 0xdc00 && text.charCodeAt(i + 1) <= 0xdfff) {
        add(4); i++;
      } else add(c >= 0xd800 && c <= 0xdfff ? 6 : 3);
    }
  }
  function count(item: unknown, depth: number) {
    if (depth > 32) throw new Error('Editor preparation is too complex');
    if (typeof item === 'string') string(item);
    else if (item === null || typeof item === 'boolean' || typeof item === 'number') add(JSON.stringify(item).length);
    else if (Array.isArray(item)) {
      add(2 + Math.max(0, item.length - 1));
      for (const child of item) count(child, depth + 1);
    } else if (typeof item === 'object') {
      const entries = Object.entries(item as Record<string, unknown>);
      add(2 + Math.max(0, entries.length - 1));
      for (const [key, child] of entries) { string(key); add(1); count(child, depth + 1); }
    } else throw new Error('Editor preparation contains unsupported values');
  }
  count(value, 0);
  return JSON.stringify(value);
}
export function sameRequest(a: FlushRequest, b: FlushRequest): boolean {
  return a.requestId === b.requestId && sameIdentity(a, b);
}
export function sameIdentity(a: FlushRequest, b: FlushRequest): boolean {
  return a.identity.ownerEpoch === b.identity.ownerEpoch && a.identity.rendererEpoch === b.identity.rendererEpoch;
}
export function validRequest(value: unknown): value is FlushRequest {
  if (!value || typeof value !== 'object') return false;
  const r = value as FlushRequest;
  const uuid = /^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$/;
  return Object.keys(r).sort().join(',') === 'identity,requestId' && typeof r.requestId === 'string' && uuid.test(r.requestId)
    && !!r.identity && typeof r.identity === 'object'
    && Object.keys(r.identity).sort().join(',') === 'ownerEpoch,rendererEpoch'
    && typeof r.identity.ownerEpoch === 'string' && typeof r.identity.rendererEpoch === 'string'
    && uuid.test(r.identity.ownerEpoch) && uuid.test(r.identity.rendererEpoch);
}
export async function submitEditor(submission: EditorSubmission): Promise<void> {
  boundedJson(submission);
  const receipt = await invoke<EditorReceipt>('native_profile_editor_submit', { submission });
  if (!receipt || !sameRequest(receipt, submission.request) || receipt.generation !== submission.generation
    || receipt.entryCount !== submission.entries.length || !/^[0-9a-f]{64}$/.test(receipt.sha256)) {
    throw new Error('Native editor preparation was not confirmed');
  }
}
