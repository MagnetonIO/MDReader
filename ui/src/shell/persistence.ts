/**
 * Session + settings persistence over `tauri-plugin-store`. Two JSON stores
 * (`session.json`, `settings.json`), written debounced by the caller and read
 * once at launch. Everything is guarded by {@link isTauri}: outside the desktop
 * shell (website, tests, SSR) these are inert no-ops, so the shell renders and
 * the stores stay in memory.
 */

import { load } from '@tauri-apps/plugin-store';
import type { Store } from '@tauri-apps/plugin-store';

import {
  deserializeSession,
  sanitizeSettings,
  type RestoredSession,
  type SessionData,
} from '../stores/session.js';
import type { SettingsSnapshot } from '../stores/settings.js';

const SESSION_KEY = 'session';
const SETTINGS_KEY = 'settings';

/** True inside a Tauri webview (the store/dialog plugins are available). */
export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

/** A single-flight, newest-wins async writer. */
export interface SerialWriter<T> {
  /** Enqueue `value`. Fire-and-forget: serialized against any in-flight write. */
  write: (value: T) => void;
  /** Resolve once no write is in flight or queued — a clean flush point. */
  drain: () => Promise<void>;
}

/**
 * Serialize calls to an async `write` so at most ONE runs at a time. Calls made
 * while a write is in flight coalesce to the **most recent** value only — for
 * session persistence intermediate snapshots are worthless, so a burst collapses
 * to a single trailing write of the latest state (newest-wins, no interleaving,
 * no lost final state). A failing write is swallowed so it can never wedge the
 * queue. {@link SerialWriter.drain} awaits the in-flight write *and* any coalesced
 * follow-up, so a close handler can guarantee the latest state hit disk.
 */
export function createSerialWriter<T>(write: (value: T) => Promise<void>): SerialWriter<T> {
  let inFlight: Promise<void> | null = null;
  let pending: { value: T } | null = null;

  function pump(value: T): void {
    inFlight = write(value)
      .catch(() => {
        /* a failed persist must not wedge the serializer */
      })
      .finally(() => {
        inFlight = null;
        if (pending) {
          const next = pending.value;
          pending = null;
          pump(next);
        }
      });
  }

  return {
    write(value) {
      if (inFlight) pending = { value };
      else pump(value);
    },
    async drain() {
      while (inFlight) await inFlight;
    },
  };
}

let sessionStoreP: Promise<Store> | undefined;
let settingsStoreP: Promise<Store> | undefined;

function sessionStore(): Promise<Store> {
  return (sessionStoreP ??= load('session.json', { autoSave: false }));
}

function settingsStore(): Promise<Store> {
  return (settingsStoreP ??= load('settings.json', { autoSave: false }));
}

export interface LoadedState {
  session: RestoredSession | null;
  settings: Partial<SettingsSnapshot>;
}

/** Read the persisted session + settings, tolerating any corruption. */
export async function loadPersisted(): Promise<LoadedState> {
  if (!isTauri()) return { session: null, settings: {} };
  try {
    const [store, settings] = [await sessionStore(), await settingsStore()];
    const rawSession = await store.get(SESSION_KEY);
    const rawSettings = await settings.get(SETTINGS_KEY);
    return { session: deserializeSession(rawSession), settings: sanitizeSettings(rawSettings) };
  } catch {
    return { session: null, settings: {} };
  }
}

export async function saveSession(data: SessionData): Promise<void> {
  if (!isTauri()) return;
  try {
    const store = await sessionStore();
    await store.set(SESSION_KEY, data);
    await store.save();
  } catch {
    /* persistence is best-effort */
  }
}

/**
 * The app's single session writer. Every session persist (debounced, on
 * visibility change, and on window close) goes through this so writes never
 * overlap and only the newest snapshot survives a burst. `drain()` before a
 * window close guarantees the final state is flushed.
 */
export const sessionWriter: SerialWriter<SessionData> = createSerialWriter(saveSession);

export async function saveSettings(snapshot: SettingsSnapshot): Promise<void> {
  if (!isTauri()) return;
  try {
    const store = await settingsStore();
    await store.set(SETTINGS_KEY, snapshot);
    await store.save();
  } catch {
    /* persistence is best-effort */
  }
}
