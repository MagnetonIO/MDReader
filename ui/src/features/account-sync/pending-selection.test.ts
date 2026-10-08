import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { pendingSelections, retainSelection, settleSelection } from './pending-selection';
const KEY = 'mdreader.selected-sync.v1';
const operation = { scope:{accountDigest:'a'.repeat(64),libraryId:'00000000-0000-4000-8000-000000000001',deviceId:'A'.repeat(43)},receipt:{documentId:'00000000-0000-4000-8000-000000000003',sourceId:'b'.repeat(64),revision:'c'.repeat(64)},operationId:'00000000-0000-4000-8000-000000000004' };
beforeEach(() => Object.defineProperty(navigator, 'locks', { configurable:true, value:{ request:vi.fn(async (_name: string, _options: unknown, action: (lock: unknown) => unknown) => action({name:KEY,mode:'exclusive'})) } }));
afterEach(() => { localStorage.clear(); vi.restoreAllMocks(); });
it('preserves malformed/future retained bytes instead of replacing an unknown operation', async () => {
  for (const raw of ['{"version":2,"operations":[]}', '{broken', '{"version":1,"version":1,"operations":[]}']) {
    localStorage.setItem(KEY,raw);
    expect(() => pendingSelections()).toThrow();
    await expect(retainSelection(operation)).rejects.toThrow();
    expect(localStorage.getItem(KEY)).toBe(raw);
  }
});
it('keeps exactly scoped immutable identities until the exact operation is reconciled', async () => {
  await retainSelection(operation);
  await retainSelection(operation);
  const exact = localStorage.getItem(KEY);
  const replacement = {...operation,operationId:'00000000-0000-4000-8000-000000000005'};
  await expect(retainSelection(replacement)).rejects.toThrow();
  await expect(settleSelection(replacement)).rejects.toThrow();
  expect(localStorage.getItem(KEY)).toBe(exact);
  const other = {...replacement,scope:{...operation.scope,accountDigest:'d'.repeat(64)}};
  await retainSelection(other);
  await settleSelection(operation);
  expect(pendingSelections()).toEqual([other]);
});

it('uses exclusive storage-only admission, elides unchanged writes and fails closed without locks', async () => {
  const writes = vi.spyOn(Storage.prototype,'setItem');
  await retainSelection(operation);
  await retainSelection(operation);
  expect(writes).toHaveBeenCalledTimes(1);
  expect(navigator.locks.request).toHaveBeenCalledWith(KEY,{mode:'exclusive',ifAvailable:true},expect.any(Function));
  const exact = localStorage.getItem(KEY);
  Object.defineProperty(navigator,'locks',{configurable:true,value:undefined});
  await expect(retainSelection(operation)).rejects.toThrow(/local reading remains available/);
  expect(localStorage.getItem(KEY)).toBe(exact);
});
