import { describe, expect, it } from 'vitest';
import { DraftJournal } from './journal';
import { sameBase, type DraftBackend, type DraftMetadata, type DraftSlot, type DraftSnapshot } from './backend';
const snapshot: DraftSnapshot = { key: { kind: 'local', source_id: '/chosen', path: '/chosen/test.md' }, base_revision: 'old', base_hash: null, title: 'test.md', markdown: '# draft', dirty: true };
function fake(initial: DraftMetadata | null = null) {
  let slot: DraftSlot = { draft_id: 'profile-id', generation: initial?.generation ?? 0, draft: initial };
  const bodies = new Map<number, string>();
  const calls: string[] = [];
  const backend: DraftBackend = async (request) => {
    calls.push(request.operation);
    if (request.operation === 'read') return { result: 'read', draft: slot.draft!, markdown: bodies.get(request.generation) ?? 'previous draft' };
    if (request.operation === 'inspect') return { result: 'slot', slot: { ...slot } };
    if (request.expected_generation !== slot.generation) throw new Error('stale');
    const generation = slot.generation + 1;
    slot = { ...slot, generation, draft: request.operation === 'discard' ? null : { ...request.draft, key: request.key, draft_id: slot.draft_id, generation, draft_hash: 'a'.repeat(64), bytes: request.draft.markdown.length, modified_ms: 1 } };
    if (request.operation === 'checkpoint') bodies.set(generation, request.draft.markdown);
    return { result: 'slot', slot: { ...slot } };
  };
  return { backend, calls, get: () => slot };
}
const previous: DraftMetadata = { ...snapshot, draft_id: 'profile-id', generation: 1, draft_hash: 'a'.repeat(64), bytes: 10, modified_ms: 1 };
describe('renderer draft recovery generations', () => {
  it('serializes dirty checkpoint then confirmed clean cleanup', async () => {
    const native = fake(), journal = new DraftJournal(native.backend);
    journal.update([snapshot]); const write = journal.flush();
    journal.update([{ ...snapshot, dirty: false }]); const clean = journal.flush();
    await Promise.all([write, clean]);
    expect(native.calls).toEqual(['inspect', 'checkpoint', 'discard']);
    expect(native.get().draft).toBeNull(); expect(native.get().generation).toBe(2);
  });
  it('drains queued checkpoints before explicit retirement', async () => {
    const native = fake(), journal = new DraftJournal(native.backend);
    journal.update([snapshot]); const write = journal.flush();
    const retire = journal.retire([snapshot]);
    await Promise.all([write, retire]);
    expect(native.get().draft).toBeNull(); expect(native.get().generation).toBe(2);
    await journal.flush(); expect(native.get().draft).toBeNull(); // async workspace retirement cannot resurrect it
    journal.resume([snapshot]); await journal.flush(); expect(native.get().draft).not.toBeNull(); // cancelled close resumes recovery
  });
  it('cleans our checkpoint when a confirmed clean buffer was evicted', async () => {
    const native = fake(), journal = new DraftJournal(native.backend);
    journal.update([snapshot]); await journal.flush();
    journal.update([]); await journal.flush();
    expect(native.get().draft).toBeNull();
  });
  it('never overwrites or discards a previous-launch draft on fresh open', async () => {
    const native = fake(previous), journal = new DraftJournal(native.backend);
    journal.update([{ ...snapshot, dirty: false }]); await journal.flush();
    journal.update([snapshot]); await journal.flush();
    expect(native.calls).toEqual(['inspect']);
    expect(journal.getState().drafts.size).toBe(1);
    expect(sameBase(previous, snapshot)).toBe(true);
    expect(sameBase({ ...previous, key: { kind: 'local', path: '/chosen/test.md', source_id: '/chosen' } }, snapshot)).toBe(true); // native JSON map ordering is irrelevant
    expect(sameBase(previous, { ...snapshot, base_revision: 'changed' })).toBe(false);
  });
  it('Done requires proof that this exact body was retained, without replacing an older recovery', async () => {
    const native = fake(previous), journal = new DraftJournal(native.backend);
    const newer = { ...snapshot, markdown: '# A newer unsaved body' };
    journal.update([newer]);
    await expect(journal.flush(newer)).rejects.toThrow('earlier recovered draft');
    expect(native.calls).toEqual(['inspect']);
    expect(native.get().draft).toEqual(previous);
    const owned = fake(), current = new DraftJournal(owned.backend);
    current.update([newer]); await current.flush(newer);
    expect(owned.get().draft?.base_revision).toBe(newer.base_revision);
  });
  it('refuses divergent buffers rather than picking a tab', async () => {
    const native = fake(), journal = new DraftJournal(native.backend);
    journal.update([snapshot, { ...snapshot, markdown: 'another draft' }]);
    await expect(journal.flush()).rejects.toThrow('two tabs');
    expect(native.calls).toEqual([]);
  });
  it('allows confirmed clean Save after checkpoint refusal and fences late writes', async () => {
    const native = fake(); let refused = 0;
    const journal = new DraftJournal(async request => {
      if (request.operation === 'checkpoint') { refused++; throw new Error('refused'); }
      return native.backend(request);
    });
    journal.update([snapshot]); await expect(journal.flush()).rejects.toThrow('refused');
    journal.update([{ ...snapshot, dirty: false }]); await journal.flush();
    expect(journal.getState().error).toBeNull(); expect(native.get().generation).toBe(1);
    await journal.retire([{ ...snapshot, dirty: false }]); expect(refused).toBe(1);
  });
  it.each(['save', 'discard'])('reconciles commit with lost reply before %s cleanup', async action => {
    const native = fake();
    const journal = new DraftJournal(async request => {
      const reply = await native.backend(request);
      if (request.operation === 'checkpoint') throw new Error('reply lost');
      return reply;
    });
    journal.update([snapshot]); await expect(journal.flush()).rejects.toThrow('reply lost');
    if (action === 'save') { journal.update([{ ...snapshot, dirty: false }]); await journal.flush(); }
    else await journal.retire([snapshot]);
    expect(native.calls).toEqual(['inspect', 'checkpoint', 'inspect', 'read', 'discard']);
    expect(native.get().draft).toBeNull(); expect(native.get().generation).toBe(2);
  });
  it('refuses retirement when uncertain checkpoint belongs to a different body', async () => {
    const native = fake();
    const journal = new DraftJournal(async request => {
      if (request.operation === 'checkpoint') {
        await native.backend({ ...request, draft: { ...request.draft, markdown: 'another writer' } });
        throw new Error('reply lost');
      }
      return native.backend(request);
    });
    journal.update([snapshot]); await expect(journal.flush()).rejects.toThrow('reply lost');
    await expect(journal.retire([snapshot])).rejects.toThrow('draft changed');
    expect(native.calls).not.toContain('discard'); expect(native.get().draft).not.toBeNull();
  });
  it('never retries a refused checkpoint against a new generation', async () => {
    const native = fake(); let refused = 0;
    const journal = new DraftJournal(async (request) => { if (request.operation === 'checkpoint') { refused++; throw new Error('stale'); } return native.backend(request); });
    journal.update([snapshot]); await expect(journal.flush()).rejects.toThrow('stale');
    await expect(journal.flush()).rejects.toThrow('needs attention'); expect(refused).toBe(1);
    expect(journal.getState().error).toContain('needs attention');
  });
});
