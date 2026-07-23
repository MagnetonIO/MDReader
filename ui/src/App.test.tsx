import { act, cleanup, render, screen, fireEvent, waitFor } from '@testing-library/react';
import { afterEach, expect, test } from 'vitest';

import App from './App';
import { detectMac } from './keymap/registry';
import { useTabsStore } from './stores/tabs';
import { useSettingsStore } from './stores/settings';

afterEach(() => {
  cleanup();
  useTabsStore.getState().reset();
  useSettingsStore.getState().reset();
});

/** Fire a portable Mod-chord keydown on window for the detected platform. */
function modKeyDown(key: string, extra: Partial<KeyboardEventInit> = {}) {
  const mac = detectMac();
  fireEvent.keyDown(window, {
    key,
    metaKey: mac,
    ctrlKey: !mac,
    ...extra,
  });
}

test('App renders the shell chrome and the reading welcome (no folder open)', async () => {
  render(<App />);
  expect(screen.getByText('MDReader')).toBeTruthy();
  expect(screen.getByText(/A calm place to read your markdown/i)).toBeTruthy();
  // The tab strip shows its empty affordance until a document is opened.
  expect(screen.getByText('No open documents')).toBeTruthy();
});

test('Mod+/ opens the shortcut sheet and Escape closes it', async () => {
  render(<App />);
  expect(screen.queryByRole('dialog', { name: /keyboard shortcuts/i })).toBeNull();

  act(() => modKeyDown('/'));
  await waitFor(() =>
    expect(screen.getByRole('dialog', { name: /keyboard shortcuts/i })).toBeTruthy(),
  );
  // The sheet is rendered straight from the keymap registry.
  expect(screen.getByText('Jump to file')).toBeTruthy();

  act(() => fireEvent.keyDown(window, { key: 'Escape' }));
  await waitFor(() =>
    expect(screen.queryByRole('dialog', { name: /keyboard shortcuts/i })).toBeNull(),
  );
});

test('Mod+\\ toggles the sidebar collapsed state', async () => {
  render(<App />);
  expect(useSettingsStore.getState().sidebarCollapsed).toBe(false);
  act(() => modKeyDown('\\'));
  await waitFor(() => expect(useSettingsStore.getState().sidebarCollapsed).toBe(true));
});
