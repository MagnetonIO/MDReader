/**
 * Reading + shell preferences. Theme drives `[data-theme]` on the document root
 * (see {@link applyTheme}); reading font and text size are surfaced as CSS custom
 * properties on the reader pane so the shared MarkdownReader restyles without any
 * change to `@mdreader/ui`.
 */

import { create } from 'zustand';

export type Theme = 'light' | 'dark' | 'system';
export type ReadingFont = 'sans' | 'serif';

/** Multipliers applied to the reader's base text size (`--text-0`). */
export const TEXT_SCALE_STEPS = [0.85, 0.9, 0.95, 1, 1.08, 1.18, 1.3] as const;
/** Index into {@link TEXT_SCALE_STEPS} representing 1.0 (no scaling). */
export const DEFAULT_TEXT_SCALE_INDEX = 3;

const THEME_CYCLE: Theme[] = ['system', 'light', 'dark'];

export interface SettingsState {
  theme: Theme;
  readingFont: ReadingFont;
  textScaleIndex: number;
  sidebarCollapsed: boolean;
  outlineVisible: boolean;
  /** Whether the reader's Related rail is expanded (persisted, collapsed by default). */
  relatedRailOpen: boolean;
  /** Whether the Inspector rail is expanded (persisted, collapsed by default). */
  inspectorOpen: boolean;
  /**
   * The schema the reader is currently interpreted through, or `null` for the
   * plain reading. Only a name is held: the schema's shape and contents are
   * always re-read from the engine, so a stale or retired name resolves to the
   * plain reading rather than to a remembered snapshot.
   */
  lensSchema: string | null;
  /**
   * The name reviews are signed with. Empty means **no reviewer is set** — the
   * app has no identity of its own to fall back on, and a review may not be
   * submitted until a person supplies one. Never defaulted, never a placeholder.
   */
  reviewer: string;
  focusMode: boolean;

  setTheme: (theme: Theme) => void;
  cycleTheme: () => void;
  setReadingFont: (font: ReadingFont) => void;
  toggleReadingFont: () => void;
  increaseTextSize: () => void;
  decreaseTextSize: () => void;
  toggleSidebar: () => void;
  setSidebarCollapsed: (collapsed: boolean) => void;
  toggleOutline: () => void;
  toggleRelatedRail: () => void;
  setRelatedRailOpen: (open: boolean) => void;
  toggleInspector: () => void;
  setInspectorOpen: (open: boolean) => void;
  setLensSchema: (schema: string | null) => void;
  /** Adopt a reviewer name. Trimmed; a blank name clears it rather than storing whitespace. */
  setReviewer: (reviewer: string) => void;
  toggleFocusMode: () => void;
  setFocusMode: (on: boolean) => void;
  hydrate: (partial: Partial<SettingsSnapshot>) => void;
  reset: () => void;
}

/** The persisted subset of the settings (no actions). */
export type SettingsSnapshot = Pick<
  SettingsState,
  | 'theme'
  | 'readingFont'
  | 'textScaleIndex'
  | 'sidebarCollapsed'
  | 'outlineVisible'
  | 'relatedRailOpen'
  | 'inspectorOpen'
  | 'lensSchema'
  | 'reviewer'
>;

const defaults: SettingsSnapshot & { focusMode: boolean } = {
  theme: 'system',
  readingFont: 'serif',
  textScaleIndex: DEFAULT_TEXT_SCALE_INDEX,
  sidebarCollapsed: false,
  outlineVisible: true,
  relatedRailOpen: false,
  inspectorOpen: false,
  lensSchema: null,
  reviewer: '',
  focusMode: false,
};

export const useSettingsStore = create<SettingsState>((set, get) => ({
  ...defaults,

  setTheme: (theme) => set({ theme }),
  cycleTheme: () =>
    set((state) => ({
      theme: THEME_CYCLE[(THEME_CYCLE.indexOf(state.theme) + 1) % THEME_CYCLE.length]!,
    })),
  setReadingFont: (readingFont) => set({ readingFont }),
  toggleReadingFont: () =>
    set((state) => ({ readingFont: state.readingFont === 'serif' ? 'sans' : 'serif' })),
  increaseTextSize: () =>
    set((state) => ({
      textScaleIndex: Math.min(state.textScaleIndex + 1, TEXT_SCALE_STEPS.length - 1),
    })),
  decreaseTextSize: () =>
    set((state) => ({ textScaleIndex: Math.max(state.textScaleIndex - 1, 0) })),
  toggleSidebar: () => set((state) => ({ sidebarCollapsed: !state.sidebarCollapsed })),
  setSidebarCollapsed: (sidebarCollapsed) => set({ sidebarCollapsed }),
  toggleOutline: () => set((state) => ({ outlineVisible: !state.outlineVisible })),
  toggleRelatedRail: () => set((state) => ({ relatedRailOpen: !state.relatedRailOpen })),
  setRelatedRailOpen: (relatedRailOpen) => set({ relatedRailOpen }),
  toggleInspector: () => set((state) => ({ inspectorOpen: !state.inspectorOpen })),
  setInspectorOpen: (inspectorOpen) => set({ inspectorOpen }),
  setLensSchema: (lensSchema) => set({ lensSchema }),
  // Trim at the door: a whitespace-only name is not an identity, and storing it
  // would let a blank review reach a backend that (rightly) refuses it.
  setReviewer: (reviewer) => set({ reviewer: reviewer.trim() }),
  toggleFocusMode: () => set((state) => ({ focusMode: !state.focusMode })),
  setFocusMode: (focusMode) => set({ focusMode }),

  hydrate: (partial) => set({ ...get(), ...partial }),
  reset: () => set({ ...defaults }),
}));

/**
 * Whether a real reviewer identity is on file. The single predicate every
 * review path consults — an approval or rejection may not be submitted while
 * this is false, and the app never substitutes a name of its own.
 */
export function hasReviewer(state: Pick<SettingsState, 'reviewer'>): boolean {
  return state.reviewer.trim().length > 0;
}

/** The current text-size multiplier. */
export function textScaleFactor(state: Pick<SettingsState, 'textScaleIndex'>): number {
  return TEXT_SCALE_STEPS[state.textScaleIndex] ?? 1;
}

/**
 * Apply a theme to the document root. `system` clears the attribute so the
 * `prefers-color-scheme` media query in the tokens governs; `light`/`dark` set
 * `[data-theme]`, which the tokens treat as authoritative over the media query.
 */
export function applyTheme(theme: Theme, root: HTMLElement = document.documentElement): void {
  if (theme === 'system') root.removeAttribute('data-theme');
  else root.setAttribute('data-theme', theme);
}
