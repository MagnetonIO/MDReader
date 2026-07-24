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
  toggleFocusMode: () => void;
  setFocusMode: (on: boolean) => void;
  hydrate: (partial: Partial<SettingsSnapshot>) => void;
  reset: () => void;
}

/** The persisted subset of the settings (no actions). */
export type SettingsSnapshot = Pick<
  SettingsState,
  'theme' | 'readingFont' | 'textScaleIndex' | 'sidebarCollapsed' | 'outlineVisible' | 'relatedRailOpen'
>;

const defaults: SettingsSnapshot & { focusMode: boolean } = {
  theme: 'system',
  readingFont: 'serif',
  textScaleIndex: DEFAULT_TEXT_SCALE_INDEX,
  sidebarCollapsed: false,
  outlineVisible: true,
  relatedRailOpen: false,
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
  toggleFocusMode: () => set((state) => ({ focusMode: !state.focusMode })),
  setFocusMode: (focusMode) => set({ focusMode }),

  hydrate: (partial) => set({ ...get(), ...partial }),
  reset: () => set({ ...defaults }),
}));

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
