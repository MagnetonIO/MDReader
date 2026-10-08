import { expect, test } from 'vitest';
import { createSerialWriter } from '../../shell/persistence';
import { emptyHistory } from './model';
import { createSessionPatchWriter, createSessionTracker, type SessionPatch } from './session-patches';
const legacy = { version: 1, root: '/retained', tabs: [], activeTabId: null, recent: [] };

test('pending and failed legacy fields survive newest-wins canonical writes until a combined acknowledgement', async () => {
  let reject!: (cause: Error) => void;
  const gate = new Promise<void>((_, fail) => { reject = fail; });
  const saved: SessionPatch[] = [];
  const writer = createSessionPatchWriter(async (patch) => {
    saved.push(patch);
    if (saved.length === 1) await gate;
  }, createSerialWriter);
  writer.write({ version: 1, root: '/first' });
  writer.write({ version: 1, readerHistory: emptyHistory() });
  writer.write({ version: 1, root: '/latest' });
  reject(new Error('synthetic failed save'));
  await writer.drain();
  expect(saved).toEqual([{ version: 1, root: '/first' }, { version: 1, root: '/latest', readerHistory: emptyHistory() }]);
  writer.write({ version: 1, activeTabId: null });
  await writer.drain();
  expect(saved[2]).toEqual({ version: 1, activeTabId: null });
});

test('mount and canonical activity emit no legacy arrays or unsupported namespace normalization', () => {
  const tracker = createSessionTracker(legacy, emptyHistory());
  expect(tracker.next({ ...legacy }, emptyHistory())).toBeNull();
  const canonical = { ...emptyHistory(), recent: [{ reference: { kind: 'local' as const, sourceId: 'a'.repeat(64), path: 'a.md', revision: 'b'.repeat(64) }, title: 'A', heading: null, position: null }] };
  expect(tracker.next(legacy, canonical)).toEqual({ version: 1, readerHistory: canonical });
  expect(tracker.next({ ...legacy, root: '/changed' }, null)).toEqual({ version: 1, root: '/changed' });
});
