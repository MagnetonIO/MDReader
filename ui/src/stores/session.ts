/**
 * Pure (de)serialization for persisted session + settings. These functions never
 * touch the filesystem — {@link ./persistence} wires them to tauri-plugin-store.
 * Deserialization is defensive: anything read back from disk is validated shape
 * by shape, and a malformed blob yields `null`/defaults rather than a crash, so a
 * corrupt `session.json` can never brick launch.
 */

import { DEFAULT_TEXT_SCALE_INDEX, TEXT_SCALE_STEPS } from './settings.js';
import type { ReadingFont, SettingsSnapshot, Theme } from './settings.js';
import type { DocRef, RecentDoc, ScrollPos, TabState } from './types.js';

export const SESSION_VERSION = 1;

/** The persisted form of a tab (identical to TabState today; versioned apart). */
export interface SerializedTab {
  id: string;
  docRef: DocRef;
  history: DocRef[];
  historyIndex: number;
  scroll: ScrollPos;
  pinned: boolean;
  originRoot: string | null;
}

export interface SessionData {
  version: number;
  root: string | null;
  activeTabId: string | null;
  tabs: SerializedTab[];
  recent: RecentDoc[];
}

export interface SessionInput {
  root: string | null;
  activeTabId: string | null;
  tabs: TabState[];
  recent: RecentDoc[];
}

/** The restored, validated session, ready to hydrate the stores. */
export interface RestoredSession {
  root: string | null;
  activeTabId: string | null;
  tabs: TabState[];
  recent: RecentDoc[];
}

export function serializeSession(input: SessionInput): SessionData {
  return {
    version: SESSION_VERSION,
    root: input.root,
    activeTabId: input.activeTabId,
    recent: input.recent.map(cloneRecent),
    tabs: input.tabs.map((tab) => ({
      id: tab.id,
      docRef: cloneRef(tab.docRef),
      history: tab.history.map(cloneRef),
      historyIndex: tab.historyIndex,
      scroll: { block: tab.scroll.block, offset: tab.scroll.offset },
      pinned: tab.pinned,
      originRoot: tab.originRoot,
    })),
  };
}

/** Validate and reconstruct a session, or `null` when the blob is unusable. */
export function deserializeSession(raw: unknown): RestoredSession | null {
  if (!isRecord(raw)) return null;
  if (raw.version !== SESSION_VERSION) return null;

  const rawTabs = Array.isArray(raw.tabs) ? raw.tabs : [];
  const tabs: TabState[] = [];
  for (const candidate of rawTabs) {
    const tab = parseTab(candidate);
    if (tab) tabs.push(tab);
  }

  const activeTabId =
    typeof raw.activeTabId === 'string' && tabs.some((tab) => tab.id === raw.activeTabId)
      ? raw.activeTabId
      : (tabs[0]?.id ?? null);

  const root = typeof raw.root === 'string' ? raw.root : null;

  // Migration-on-load: a legacy session (pre-`originRoot`) restores its tabs with
  // a null origin. Left null, `retireForeign` would treat them as foreign to the
  // very workspace they belong to, so a same-root reopen would wrongly retire the
  // restored tabs. Adopt the session's own root as their origin here.
  if (root !== null) {
    for (const tab of tabs) {
      if (tab.originRoot === null) tab.originRoot = root;
    }
  }

  return {
    root,
    activeTabId,
    tabs,
    // Legacy blobs stored recents without a workspace root; fall back to the
    // session's own root so they stay visible under the last-open workspace.
    recent: parseRecentArray(raw.recent, root),
  };
}

function parseTab(candidate: unknown): TabState | null {
  if (!isRecord(candidate)) return null;
  if (typeof candidate.id !== 'string') return null;
  const docRef = parseRef(candidate.docRef);
  if (!docRef) return null;

  const history = parseRefArray(candidate.history);
  const safeHistory = history.length > 0 ? history : [docRef];
  const rawIndex = typeof candidate.historyIndex === 'number' ? candidate.historyIndex : 0;
  const historyIndex = clamp(rawIndex, 0, safeHistory.length - 1);

  return {
    id: candidate.id,
    docRef: safeHistory[historyIndex] ?? docRef,
    history: safeHistory,
    historyIndex,
    scroll: parseScroll(candidate.scroll),
    pinned: candidate.pinned === true,
    // Legacy blobs predate per-tab origin; a null origin makes the tab foreign
    // to any real root, so it retires (attributed to session root by the caller).
    originRoot: typeof candidate.originRoot === 'string' ? candidate.originRoot : null,
  };
}

function parseRef(candidate: unknown): DocRef | null {
  if (!isRecord(candidate)) return null;
  if (typeof candidate.path !== 'string' || typeof candidate.name !== 'string') return null;
  return { path: candidate.path, name: candidate.name };
}

function parseRefArray(candidate: unknown): DocRef[] {
  if (!Array.isArray(candidate)) return [];
  const refs: DocRef[] = [];
  for (const entry of candidate) {
    const ref = parseRef(entry);
    if (ref) refs.push(ref);
  }
  return refs;
}

function parseRecentArray(candidate: unknown, fallbackRoot: string | null): RecentDoc[] {
  if (!Array.isArray(candidate)) return [];
  const recents: RecentDoc[] = [];
  for (const entry of candidate) {
    const ref = parseRef(entry);
    if (!ref) continue;
    const root =
      isRecord(entry) && typeof entry.root === 'string' ? entry.root : fallbackRoot;
    recents.push({ path: ref.path, name: ref.name, root });
  }
  return recents;
}

function parseScroll(candidate: unknown): ScrollPos {
  if (!isRecord(candidate)) return { block: 0, offset: 0 };
  return {
    block: typeof candidate.block === 'number' ? candidate.block : 0,
    offset: typeof candidate.offset === 'number' ? candidate.offset : 0,
  };
}

// --- settings ---

const THEMES: readonly Theme[] = ['system', 'light', 'dark'];
const FONTS: readonly ReadingFont[] = ['sans', 'serif'];

/** Coerce a raw settings blob into a valid partial snapshot (drops bad fields). */
export function sanitizeSettings(raw: unknown): Partial<SettingsSnapshot> {
  if (!isRecord(raw)) return {};
  const out: Partial<SettingsSnapshot> = {};
  if (isOneOf(raw.theme, THEMES)) out.theme = raw.theme;
  if (isOneOf(raw.readingFont, FONTS)) out.readingFont = raw.readingFont;
  if (
    typeof raw.textScaleIndex === 'number' &&
    Number.isInteger(raw.textScaleIndex) &&
    raw.textScaleIndex >= 0 &&
    raw.textScaleIndex < TEXT_SCALE_STEPS.length
  ) {
    out.textScaleIndex = raw.textScaleIndex;
  }
  if (typeof raw.sidebarCollapsed === 'boolean') out.sidebarCollapsed = raw.sidebarCollapsed;
  if (typeof raw.outlineVisible === 'boolean') out.outlineVisible = raw.outlineVisible;
  if (typeof raw.relatedRailOpen === 'boolean') out.relatedRailOpen = raw.relatedRailOpen;
  return out;
}

export function serializeSettings(snapshot: SettingsSnapshot): SettingsSnapshot {
  return {
    theme: snapshot.theme,
    readingFont: snapshot.readingFont,
    textScaleIndex: snapshot.textScaleIndex,
    sidebarCollapsed: snapshot.sidebarCollapsed,
    outlineVisible: snapshot.outlineVisible,
    relatedRailOpen: snapshot.relatedRailOpen,
  };
}

/** A defaulted settings snapshot for a fresh install. */
export const DEFAULT_SETTINGS: SettingsSnapshot = {
  theme: 'system',
  readingFont: 'serif',
  textScaleIndex: DEFAULT_TEXT_SCALE_INDEX,
  sidebarCollapsed: false,
  outlineVisible: true,
  relatedRailOpen: false,
};

// --- helpers ---

function cloneRef(ref: DocRef): DocRef {
  return { path: ref.path, name: ref.name };
}

function cloneRecent(ref: RecentDoc): RecentDoc {
  return { path: ref.path, name: ref.name, root: ref.root };
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null;
}

function isOneOf<T extends string>(value: unknown, options: readonly T[]): value is T {
  return typeof value === 'string' && (options as readonly string[]).includes(value);
}

function clamp(value: number, min: number, max: number): number {
  return Math.min(Math.max(value, min), max);
}
