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
