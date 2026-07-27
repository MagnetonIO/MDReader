/** Overlay titlebar: the app's single drag region, with the sidebar toggle at the
 * leading edge (inset past the macOS traffic lights) and the reading controls at
 * the trailing edge. Kept airy — this is chrome, not a cockpit. */

import { FocusIcon, PanelLeft, SettingsIcon, ThemeIcon, TypeIcon } from './icons.js';
import type { ReadingFont, Theme } from '../stores/settings.js';
import { UpdateControl } from './UpdateControl.js';

export interface TitlebarProps {
  isMac: boolean;
  sidebarCollapsed: boolean;
  onToggleSidebar: () => void;
  theme: Theme;
  onCycleTheme: () => void;
  readingFont: ReadingFont;
  onToggleFont: () => void;
  onDecreaseText: () => void;
  onIncreaseText: () => void;
  onToggleFocus: () => void;
  onShowSettings: () => void;
  onShowShortcuts: () => void;
}

const THEME_LABEL: Record<Theme, string> = { system: 'System', light: 'Light', dark: 'Dark' };

export function Titlebar({
  sidebarCollapsed,
  onToggleSidebar,
  theme,
  onCycleTheme,
  readingFont,
  onToggleFont,
  onDecreaseText,
  onIncreaseText,
  onToggleFocus,
  onShowSettings,
  onShowShortcuts,
}: TitlebarProps) {
  return (
    <header className="titlebar">
      <button
        type="button"
        className={`icon-btn${sidebarCollapsed ? '' : ' icon-btn--active'}`}
        aria-pressed={!sidebarCollapsed}
        onClick={onToggleSidebar}
        title="Toggle sidebar (Cmd+\\)"
        aria-label="Toggle sidebar"
      >
        <PanelLeft width="15" height="15" />
      </button>
      <span className="titlebar__title" data-tauri-drag-region>MDReader</span>
      <span className="titlebar__spacer" data-tauri-drag-region />
      <UpdateControl />

      <button
        type="button"
        className="icon-btn"
        onClick={onToggleFont}
        title={`Reading font: ${readingFont === 'serif' ? 'Serif' : 'Sans'}`}
      >
        <TypeIcon width="15" height="15" />
        {readingFont === 'serif' ? 'Serif' : 'Sans'}
      </button>
      <button type="button" className="icon-btn" onClick={onDecreaseText} title="Smaller text" aria-label="Smaller text">
        A−
      </button>
      <button type="button" className="icon-btn" onClick={onIncreaseText} title="Larger text" aria-label="Larger text">
        A+
      </button>
      <button
        type="button"
        className="icon-btn"
        onClick={onCycleTheme}
        title={`Theme: ${THEME_LABEL[theme]}`}
        aria-label="Cycle theme"
      >
        <ThemeIcon width="15" height="15" />
      </button>
      <button
        type="button"
        className="icon-btn"
        onClick={onToggleFocus}
        title="Focus mode (Cmd+.)"
        aria-label="Focus mode"
      >
        <FocusIcon width="15" height="15" />
      </button>
      <button
        type="button"
        className="icon-btn"
        onClick={onShowSettings}
        title="Settings (Cmd+,)"
        aria-label="Settings"
      >
        <SettingsIcon width="15" height="15" />
      </button>
      <button
        type="button"
        className="icon-btn"
        onClick={onShowShortcuts}
        title="Keyboard shortcuts (Cmd+/)"
        aria-label="Keyboard shortcuts"
      >
        ?
      </button>
    </header>
  );
}
