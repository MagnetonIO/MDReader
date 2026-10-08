/** Renderer editor admission only. A hold is not a native preservation proof. */
import type { EditorEntry } from '../../stores/editor';

type Entries = Record<string, EditorEntry>;
interface Stamp { incarnation: object; generation: number }
export interface EditorSave {
  readonly entry: Readonly<EditorEntry>;
  saved: (hash: string) => boolean;
  conflicted: (hash: string) => boolean;
  /** Release only after the real save settles, including unknown/error replies. */
  finish: () => void;
}
export interface EditorHold {
  /** Join issued work; releasing this hold never cancels an underlying save. */
  drain: () => Promise<void>;
  generation: () => number | null;
  /** Entire registered set, only after issued saves have settled. */
  snapshot: (entries: Entries) => { generation: string; entries: Array<EditorEntry & { entryId: string }> };
  release: () => boolean;
}

export function createEditorAdmission() {
  let generation = 0;
  let held: object | null = null;
  let exhausted = false;
  const stamps = new Map<string, Stamp>();
  const saves = new Map<string, { identity: object; completion: Promise<void> }>();
  const listeners = new Set<() => void>();
  function advance() {
    if (generation === Number.MAX_SAFE_INTEGER) { exhausted = true; notify(); }
    else generation += 1;
  }
  const denied = () => held !== null || exhausted;
  function notify() { for (const listener of listeners) listener(); }
  return {
    denied,
    subscribe(listener: () => void) { listeners.add(listener); return () => { listeners.delete(listener); }; },
    /** The store calls this for every actual mutation, including lease settlement. */
    changed(before: Entries, after: Entries, replaced?: string) {
      if (before === after) return;
      advance();
      for (const key of Object.keys(before)) if (!(key in after)) stamps.delete(key);
      for (const [key, entry] of Object.entries(after)) {
        if (before[key] === entry && key !== replaced) continue;
        const previous = stamps.get(key);
        stamps.set(key, { incarnation: previous && key !== replaced ? previous.incarnation : {}, generation });
      }
    },
    hold(): EditorHold | null {
      if (denied()) return null;
      const identity = {};
      held = identity;
      notify();
      return {
        async drain() {
          if (held !== identity || exhausted) throw new Error('Editor admission is no longer held');
          // No new save can enter while this hold is current. Issued effects
          // remain owned until their actual finally path signals completion.
          await Promise.all([...saves.values()].map((save) => save.completion));
          if (held !== identity || exhausted) throw new Error('Editor admission is no longer held');
        },
        generation: () => held === identity && !exhausted ? generation : null,
        snapshot(entries) {
          if (held !== identity || exhausted || saves.size) throw new Error('Editor work is not drained');
          const values = Object.entries(entries);
          if (values.length > 64) throw new Error('Too many editor buffers to prepare');
          return { generation: String(generation), entries: values.map(([entryId, entry]) => {
            if (entryId !== `${entry.tabId} ${entry.path}`) throw new Error('Editor registration changed');
            return { ...entry, entryId, conflict: entry.conflict ? { ...entry.conflict } : null };
          }) };
        },
        release() {
          if (held !== identity) return false;
          held = null; notify(); return true;
        },
      };
    },
    save(key: string, entry: EditorEntry, current: () => EditorEntry | undefined, commit: (entry: EditorEntry) => void): EditorSave | null {
      if (denied() || saves.has(key) || saves.size >= 64) return null;
      let stamp = stamps.get(key);
      if (!stamp) { stamp = { incarnation: {}, generation }; stamps.set(key, stamp); }
      const captured = stamp;
      const identity = {};
      let complete!: () => void;
      const completion = new Promise<void>((resolve) => { complete = resolve; });
      saves.set(key, { identity, completion });
      advance();
      const snapshot = Object.freeze({ ...entry, conflict: entry.conflict ? Object.freeze({ ...entry.conflict }) : null });
      let settled = false;
      function settle(update: (live: EditorEntry) => EditorEntry): boolean {
        if (settled || saves.get(key)?.identity !== identity || exhausted) return false;
        settled = true;
        const live = current();
        const latest = stamps.get(key);
        if (!live || latest?.incarnation !== captured.incarnation) return false;
        // Ordinary typing during a save keeps existing behavior. Under a hold,
        // only the exact captured entry generation may adopt its completion.
        if (held && latest.generation !== captured.generation) return false;
        commit(update(live));
        return true;
      }
      return {
        entry: snapshot,
        saved(hash) {
          return settle((live) => ({ ...live, savedContent: snapshot.buffer, baseHash: hash,
            dirty: live.buffer !== snapshot.buffer, conflict: null }));
        },
        conflicted(hash) {
          return settle((live) => ({ ...live, conflict: { currentHash: hash, source: 'save', phase: 'choose' } }));
        },
        finish() {
          if (saves.get(key)?.identity !== identity) return;
          saves.delete(key); advance(); complete();
        },
      };
    },
  };
}

/** Shared by the real editor store, save entry points and CodeMirror filter. */
export const editorAdmission = createEditorAdmission();
