import { expect, test } from 'vitest';
import { createSerialWriter } from '../../shell/persistence';
import { emptyHistory } from './model';
import { restoreReaderHistory } from './serialization';
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

test('snapshot uses pending and failed groups without consuming changes or normalizing a readonly namespace', async () => {
  const raw = '{"version":1,"readerHistory":{"version":99,"unknown":9007199254740993},"tabs":[{"unknown":1e+40}]}';
  const restored = restoreReaderHistory((JSON.parse(raw) as { readerHistory: unknown }).readerHistory, true);
  expect(restored.mode).toBe('readonly');
  const tracker = createSessionTracker(legacy, emptyHistory());
  let reject!: (cause: Error) => void;
  const gate = new Promise<void>((_, fail) => { reject = fail; });
  let fail = true;
  const writer = createSessionPatchWriter(async () => {
    if (fail) { await gate; throw new Error('synthetic failed save'); }
  }, createSerialWriter);
  writer.write({ version: 1, root: '/pending' });
  writer.write({ version: 1, readerHistory: emptyHistory() });
  const current = { ...legacy, root: '/current' };
  const peek = tracker.peek(current, restored.mode === 'native_atomic' ? restored.data : null);
  const snapshot = writer.snapshot(peek, restored.mode === 'native_atomic');
  expect(snapshot).toEqual({ version: 1, root: '/current' });
  expect(tracker.peek(current, null)).toEqual(peek);
  snapshot.root = '/caller-mutation';
  const writable = writer.snapshot(null, true);
  writable.readerHistory!.recent.push({ reference: { kind: 'local', sourceId: 'a'.repeat(64), path: 'x.md', revision: 'b'.repeat(64) }, title: 'X', heading: null, position: null });
  expect(writer.snapshot(null, true)).toEqual({ version: 1, root: '/pending', readerHistory: emptyHistory() });
  reject(new Error('synthetic interrupted write'));
  await expect(writer.drain()).rejects.toThrow('synthetic interrupted write');
  expect(writer.snapshot(null, false)).toEqual({ version: 1, root: '/pending' });
  expect(tracker.next(current, null)).toEqual(peek);
  expect(tracker.peek(current, null)).toBeNull();
  fail = false;
  writer.write(peek!);
  await writer.drain();
  expect(writer.snapshot(null, true)).toEqual({ version: 1 });
  expect(raw).toBe('{"version":1,"readerHistory":{"version":99,"unknown":9007199254740993},"tabs":[{"unknown":1e+40}]}');
});
