import { draftKeyId, type DraftBackend, type DraftMetadata, type DraftSlot, type DraftSnapshot } from './backend';

interface RecordState { slot: DraftSlot; owned: boolean; last: string | null; blocked: boolean; retired: string | null }
export interface RecoveryState { drafts: ReadonlyMap<string, DraftMetadata>; error: string | null }

/** One bounded serial writer per renderer. Native generations fence other owners
 * and late writes; an existing recovery is never overwritten by a newly opened editor. */
export class DraftJournal {
  private records = new Map<string, RecordState>();
  private snapshots: DraftSnapshot[] = [];
  private queue = Promise.resolve();
  private listeners = new Set<() => void>();
  private state: RecoveryState = { drafts: new Map(), error: null };
  constructor(private backend: DraftBackend) {}
  getState = () => this.state;
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  update(snapshots: DraftSnapshot[]) { this.snapshots = snapshots; }
  private publish(error: string | null = this.state.error) {
    const blocked = this.snapshots.some(snapshot => snapshot.dirty && this.records.get(draftKeyId(snapshot.key))?.blocked);
    this.state = { drafts: new Map([...this.records].flatMap(([key, record]) =>
      record.slot.draft && !record.owned ? [[key, record.slot.draft] as const] : [])),
      error: blocked ? 'Automatic draft recovery needs attention. Save your changes or a copy before closing MDReader.' : error };
    for (const listener of this.listeners) listener();
  }
  private run<T>(action: () => Promise<T>): Promise<T> {
    const task = this.queue.then(action);
    this.queue = task.then(() => {}, (cause) => { this.publish(cause instanceof Error ? cause.message : 'Automatic draft recovery is unavailable. Save a copy to keep your changes.'); });
    return task;
  }
  private async inspect(key: DraftSnapshot['key']): Promise<RecordState> {
    const id = draftKeyId(key);
    const existing = this.records.get(id);
    if (existing) return existing;
    const reply = await this.backend({ operation: 'inspect', key });
    if (reply.result !== 'slot') throw new Error('Draft recovery could not be checked.');
    const record = { slot: reply.slot, owned: false, last: null, blocked: false, retired: null };
    this.records.set(id, record); this.publish(); return record;
  }
  flush(): Promise<void> {
    // Capture a complete generation before queueing. A later clean generation
    // follows it and discards only after any earlier checkpoint has settled.
    const snapshots = this.snapshots.slice();
    return this.run(async () => {
      const unique = new Map<string, DraftSnapshot>();
      for (const snapshot of snapshots) {
        const id = draftKeyId(snapshot.key), previous = unique.get(id);
        if (previous && (previous.markdown !== snapshot.markdown || previous.base_revision !== snapshot.base_revision))
          throw new Error('This document has different drafts in two tabs. Save a copy of each to keep them.');
        unique.set(id, snapshot);
      }
      for (const snapshot of unique.values()) {
        const record = await this.inspect(snapshot.key);
        const retirement = JSON.stringify(snapshot);
        if (record.retired === retirement) continue;
        record.retired = null;
        if (record.blocked) throw new Error('Automatic draft recovery needs attention. Save your changes or a copy before closing MDReader.');
        if (record.slot.draft && !record.owned) continue;
        if (!snapshot.dirty) {
          if (record.owned && record.slot.draft) {
            const reply = await this.backend({ operation: 'discard', draft_id: record.slot.draft_id, expected_generation: record.slot.generation });
            if (reply.result !== 'slot') throw new Error('Saved draft cleanup could not be confirmed.');
            record.slot = reply.slot; record.last = null; record.owned = false;
          }
          continue;
        }
        const fingerprint = JSON.stringify(snapshot);
        if (record.last === fingerprint) continue;
        try {
          const key = snapshot.key;
          const draft = { base_revision: snapshot.base_revision, base_hash: snapshot.base_hash, title: snapshot.title, markdown: snapshot.markdown };
          const reply = await this.backend({ operation: 'checkpoint', key, expected_generation: record.slot.generation, draft });
          if (reply.result !== 'slot' || !reply.slot.draft) throw new Error('Draft recovery could not be confirmed.');
          record.slot = reply.slot; record.owned = true; record.last = fingerprint;
        } catch (cause) {
          // A refused CAS is not a license to adopt another writer's generation.
          record.blocked = true; throw cause;
        }
      }
      // Clean buffers can be evicted before the debounce fires. They no longer
      // belong to a live editor, so retire our own checkpoints after queued writes.
      for (const [id, record] of this.records) {
        if (!unique.has(id) && record.owned && record.slot.draft) await this.clear(record);
      }
      this.publish(null);
    });
  }
  private async clear(record: RecordState) {
    const reply = await this.backend({ operation: 'discard', draft_id: record.slot.draft_id, expected_generation: record.slot.generation });
    if (reply.result !== 'slot') throw new Error('Draft cleanup could not be confirmed. Your editor is kept.');
    record.slot = reply.slot; record.owned = false; record.last = null; record.blocked = false;
  }
  /** Ordered after issued checkpoints, before the user-approved buffer removal.
   * Previous-launch drafts not accepted by this editor are left untouched. */
  retire(snapshots: DraftSnapshot[]): Promise<void> {
    return this.run(async () => {
      for (const snapshot of snapshots) {
        const record = this.records.get(draftKeyId(snapshot.key));
        if (record?.owned && record.slot.draft) await this.clear(record);
        if (record) record.retired = JSON.stringify(snapshot);
      }
      this.publish(null);
    });
  }
  resume(snapshots: DraftSnapshot[]) {
    for (const snapshot of snapshots) {
      const record = this.records.get(draftKeyId(snapshot.key));
      if (record) record.retired = null;
    }
  }
  read(draft: DraftMetadata): Promise<string> {
    return this.run(async () => {
      const reply = await this.backend({ operation: 'read', draft_id: draft.draft_id, generation: draft.generation });
      if (reply.result !== 'read') throw new Error('The recovered draft is unavailable.');
      return reply.markdown;
    });
  }
  accept(draft: DraftMetadata) {
    const record = this.records.get(draftKeyId(draft.key));
    if (record?.slot.generation === draft.generation) { record.owned = true; record.last = null; this.publish(null); }
  }
  discard(draft: DraftMetadata): Promise<void> {
    return this.run(async () => {
      const record = this.records.get(draftKeyId(draft.key));
      if (!record || record.slot.generation !== draft.generation) throw new Error('This recovered draft changed. Reopen the document before choosing again.');
      const reply = await this.backend({ operation: 'discard', draft_id: draft.draft_id, expected_generation: draft.generation });
      if (reply.result !== 'slot') throw new Error('The recovered draft could not be discarded.');
      record.slot = reply.slot; record.owned = false; record.last = null; record.blocked = false; this.publish(null);
    });
  }
}
