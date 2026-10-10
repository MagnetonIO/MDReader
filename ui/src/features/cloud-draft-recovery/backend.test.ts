import { createHash, webcrypto } from 'node:crypto';
import { afterEach, expect, test, vi } from 'vitest';
import { invoke } from '@tauri-apps/api/core';
import { cloudRecoveryBackend, type RecoveredDraft } from './backend';
vi.mock('@tauri-apps/api/core', () => ({ invoke: vi.fn() }));
const markdown = '\uFEFF# Public draft\r\n';
const bytes = new TextEncoder().encode(markdown);
const draft: RecoveredDraft = { draftId: '11111111-1111-4111-8111-111111111111', revision: createHash('sha256').update(bytes).digest('hex'), title: 'Public draft.md', sourceName: 'Previous cloud connection', state: 'unknown', byteLength: bytes.length };
afterEach(() => { vi.resetAllMocks(); vi.unstubAllGlobals(); });
test('list only requests bounded local metadata and read verifies exact body bytes before export eligibility', async () => {
  vi.stubGlobal('crypto', { subtle: webcrypto.subtle });
  vi.mocked(invoke).mockResolvedValueOnce({ operation: 'recovered_drafts', drafts: [draft], offset: 0, nextOffset: null });
  expect((await cloudRecoveryBackend.list(0)).drafts).toEqual([draft]);
  expect(invoke).toHaveBeenNthCalledWith(1, 'native_provider_request', { request: { operation: 'recovered_drafts', offset: 0, limit: 32 } });
  vi.mocked(invoke).mockResolvedValueOnce({ operation: 'read_recovered_draft', draft, markdown });
  expect(await cloudRecoveryBackend.read(draft)).toEqual({ operation: 'read_recovered_draft', draft, markdown });
  expect(invoke).toHaveBeenNthCalledWith(2, 'native_provider_request', { request: { operation: 'read_recovered_draft', draftId: draft.draftId, revision: draft.revision } });
});
test('changed hash or mismatched exact draft ID fails without any provider invocation', async () => {
  vi.stubGlobal('crypto', { subtle: webcrypto.subtle });
  vi.mocked(invoke).mockResolvedValue({ operation: 'read_recovered_draft', draft, markdown: markdown.replace('Public', 'public') });
  await expect(cloudRecoveryBackend.read(draft)).rejects.toThrow('changed');
  vi.mocked(invoke).mockResolvedValue({ operation: 'read_recovered_draft', draft: { ...draft, draftId: '22222222-2222-4222-8222-222222222222' }, markdown });
  await expect(cloudRecoveryBackend.read(draft)).rejects.toThrow('verified');
  expect(invoke).toHaveBeenCalledTimes(2);
});
test('invalid metadata and oversized listing fail closed', async () => {
  vi.mocked(invoke).mockResolvedValue({ operation: 'recovered_drafts', drafts: [{ ...draft, byteLength: 2 * 1024 * 1024 + 1 }], offset: 0, nextOffset: null });
  await expect(cloudRecoveryBackend.list(0)).rejects.toThrow('verified');
  vi.mocked(invoke).mockResolvedValue({ operation: 'recovered_drafts', drafts: Array.from({ length: 33 }, () => draft), offset: 0, nextOffset: null });
  await expect(cloudRecoveryBackend.list(0)).rejects.toThrow('verified');
});
