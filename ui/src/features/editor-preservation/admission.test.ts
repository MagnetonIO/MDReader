import { afterEach, expect, test } from 'vitest';
import { useEditorStore } from '../../stores/editor';
import { editorAdmission, type EditorHold, type EditorSave } from './admission';

const store = () => useEditorStore.getState();
const held: EditorHold[] = [];
const issued: EditorSave[] = [];
function hold() { const value = editorAdmission.hold()!; held.push(value); return value; }
function save(tab = 'tab', path = '/synthetic/a.md') {
  const value = store().beginSave(tab, path)!; issued.push(value); return value;
}
function seed(tab = 'tab', path = '/synthetic/a.md') {
  store().seed(tab, path, 'original', 'h0', 'source'); store().edit(tab, path, 'edited');
}
afterEach(() => {
  for (const value of issued.splice(0)) value.finish();
  for (const value of held.splice(0)) value.release();
  store().reset();
});

test('real store admission retains every buffer and rejects all ordinary mutations until exact release', async () => {
  seed(); seed('inactive', '/synthetic/inactive.md');
  store().raiseConflict('tab', '/synthetic/a.md', 'disk', 'external');
  const before = store().entries;
  const gate = hold(); const generation = gate.generation();
  expect(editorAdmission.hold()).toBeNull();
  store().seed('tab', '/synthetic/a.md', 'replacement', 'h1', 'read');
  store().edit('tab', '/synthetic/a.md', 'blocked');
  store().setMode('tab', '/synthetic/a.md', 'read');
  store().markSaved('tab', '/synthetic/a.md', 'blocked', 'h1');
  store().raiseConflict('tab', '/synthetic/a.md', 'blocked', 'save');
  store().toConfirmOverwrite('tab', '/synthetic/a.md');
  store().clearConflict('tab', '/synthetic/a.md');
  store().removeTab('inactive'); store().retain(() => false); store().reset();
  expect(store().beginSave('tab', '/synthetic/a.md')).toBeNull();
  expect(store().entries).toBe(before);
  expect(gate.generation()).toBe(generation);
  await gate.drain();
  expect(gate.release()).toBe(true); expect(gate.release()).toBe(false);
  const next = hold();
  expect(gate.generation()).toBeNull();
  await expect(gate.drain()).rejects.toThrow('no longer held');
  expect(gate.release()).toBe(false); expect(editorAdmission.denied()).toBe(true);
  next.release();
  store().edit('tab', '/synthetic/a.md', 'ordinary edit');
  expect(store().entry('tab', '/synthetic/a.md')!.buffer).toBe('ordinary edit');
});

test('hold drains issued ownership through actual completion; only the exact captured generation may settle', async () => {
  seed(); seed('second', '/synthetic/second.md');
  const exact = save(); const stale = save('second', '/synthetic/second.md');
  store().edit('second', '/synthetic/second.md', 'typed while saving');
  const gate = hold(); const generation = gate.generation();
  let drained = false;
  const draining = gate.drain().then(() => { drained = true; });
  expect(exact.saved('h1')).toBe(true);
  expect(exact.saved('duplicate')).toBe(false);
  expect(stale.saved('must-not-adopt')).toBe(false);
  expect(store().entry('second', '/synthetic/second.md')).toMatchObject({ buffer: 'typed while saving', baseHash: 'h0', dirty: true });
  expect(gate.generation()).toBeGreaterThan(generation!);
  await Promise.resolve(); expect(drained).toBe(false);
  exact.finish(); await Promise.resolve(); expect(drained).toBe(false);
  stale.finish(); await draining; expect(drained).toBe(true);
  expect(store().entry('tab', '/synthetic/a.md')).toMatchObject({ buffer: 'edited', baseHash: 'h1', dirty: false });
});

test('ordinary typing retains newer work and replaced-entry/finished save receipts never settle a new incarnation', () => {
  seed();
  const ordinary = save();
  expect(store().beginSave('tab', '/synthetic/a.md')).toBeNull();
  store().edit('tab', '/synthetic/a.md', 'newer');
  expect(ordinary.saved('h1')).toBe(true); ordinary.finish();
  expect(store().entry('tab', '/synthetic/a.md')).toMatchObject({ buffer: 'newer', savedContent: 'edited', baseHash: 'h1', dirty: true });
  const old = save();
  store().removeTab('tab'); seed();
  expect(old.conflicted('stale disk')).toBe(false);
  old.finish();
  const current = save(); old.finish();
  expect(old.saved('stale hash')).toBe(false);
  expect(current.saved('h2')).toBe(true);
  expect(store().entry('tab', '/synthetic/a.md')!.baseHash).toBe('h2');
});

test('releasing a hold never abandons issued work or lets its stale drain prove a replacement hold', async () => {
  seed();
  const pending = save();
  const old = hold();
  const oldDrain = expect(old.drain()).rejects.toThrow('no longer held');
  expect(old.release()).toBe(true);
  const current = hold();
  let drained = false;
  const currentDrain = current.drain().then(() => { drained = true; });
  await Promise.resolve(); expect(drained).toBe(false);
  // Model an unknown/error IPC outcome: do not manufacture a saved receipt.
  pending.finish();
  await oldDrain; await currentDrain;
  expect(old.release()).toBe(false);
  expect(editorAdmission.denied()).toBe(true);
  expect(store().entry('tab', '/synthetic/a.md')).toMatchObject({ buffer: 'edited', baseHash: 'h0', dirty: true });
});
