import { act, cleanup, render, screen, fireEvent, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';

import App from './App';
import { detectMac } from './keymap/registry';
import { useTabsStore } from './stores/tabs';
import { useSourcesStore } from './stores/sources';
import { useSettingsStore } from './stores/settings';
import { sessionWriter } from './shell/persistence';
import { nativeProvidersBackend } from './platform/providers';

afterEach(() => {
  cleanup();
  vi.restoreAllMocks();
  useTabsStore.getState().reset();
  useSettingsStore.getState().reset();
  useSourcesStore.getState().reset();
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

test('Mod+, opens Settings and Escape returns focus to the shell', async () => {
  render(<App />);
  expect(screen.queryByRole('dialog', { name: 'Settings' })).toBeNull();

  act(() => modKeyDown(','));
  await waitFor(() => expect(screen.getByRole('dialog', { name: 'Settings' })).toBeTruthy());
  expect(screen.getByRole('heading', { name: 'Library' })).toBeTruthy();
  expect(screen.getByRole('heading', { name: 'Diagnostics' })).toBeTruthy();
  const readerContent = document.querySelector('.shell__reader-content');
  expect(readerContent?.hasAttribute('inert')).toBe(true);
  expect(readerContent?.getAttribute('aria-hidden')).toBe('true');

  act(() => fireEvent.keyDown(window, { key: 'Escape' }));
  await waitFor(() => expect(screen.queryByRole('dialog', { name: 'Settings' })).toBeNull());
  expect(readerContent?.hasAttribute('inert')).toBe(false);
  expect(readerContent?.hasAttribute('aria-hidden')).toBe(false);
});

test('selected cloud revision opens in the shared reader and returns to source controls', async () => {
  const projection = { jobId: 'synthetic-job', readerSourceId: `provider:${'a'.repeat(64)}`, path: `records/${'b'.repeat(64)}.md`, revision: 'c'.repeat(64) };
  vi.spyOn(nativeProvidersBackend, 'state').mockResolvedValue({ availability: [], pendingAdmissions: [], error: null, sources: [{ sourceId: 'selected-source', readerSourceId: projection.readerSourceId, name: 'Selected plans', kind: 'google_drive', enabled: true, selectedScope: { kind: 'exact_documents', count: 1 }, capabilities: { read: true, list: true, changes: true, assets: false, guardedWrite: false }, health: 'ready', coverage: { kind: 'complete_selected', observedAtMs: 1 } }] });
  vi.spyOn(nativeProvidersBackend, 'documents').mockResolvedValue({ operation: 'documents', offset: 0, nextOffset: null, documents: [{ documentId: 'opaque-handle', name: 'Plan.md', displayPath: 'Plan.md', modifiedAtMs: null, sizeBytes: 20, cached: true, cachedObservedAtMs: 1, admitted: true, projection }] });
  vi.spyOn(nativeProvidersBackend, 'readCached').mockResolvedValue(projection);
  const read = vi.spyOn(nativeProvidersBackend, 'readDocument').mockResolvedValue({ operation: 'read_document', sourceId: projection.readerSourceId, path: projection.path, revision: projection.revision, markdown: '# Provider reading\n\nSelected original.', lineStart: 1, lineEnd: 3 });
  render(<App />);
  act(() => modKeyDown(','));
  fireEvent.click(await screen.findByRole('button', { name: 'Manage cloud sources' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Browse documents' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Read cached revision' }));
  expect(await screen.findByRole('heading', { name: /Provider reading/ })).toBeTruthy();
  expect(screen.queryByRole('dialog', { name: 'Settings' })).toBeNull();
  expect(document.querySelector('.shell__reader-content')?.hasAttribute('inert')).toBe(true);
  expect(read).toHaveBeenCalledWith(expect.objectContaining({ sourceId: projection.readerSourceId, revision: projection.revision }));
  fireEvent.click(screen.getByRole('button', { name: 'Archived reviews…' }));
  expect(await screen.findByRole('dialog', { name: 'Archived document reviews' })).toBeTruthy();
  expect(screen.queryByRole('dialog', { name: 'Cloud document' })).toBeNull();
  expect(document.querySelector('.shell__reader-content')?.hasAttribute('inert')).toBe(true);
  expect(screen.getByText('No archive selected.')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Close archived reviews' }));
  expect(await screen.findByRole('heading', { name: /Provider reading/ })).toBeTruthy();
  expect(read).toHaveBeenLastCalledWith(expect.objectContaining({ sourceId: projection.readerSourceId, revision: projection.revision }));
  fireEvent.click(screen.getByRole('button', { name: 'Back to sources' }));
  expect(await screen.findByRole('dialog', { name: 'Settings' })).toBeTruthy();
  expect(screen.queryByRole('dialog', { name: 'Cloud document' })).toBeNull();
});

test('Mod+\\ toggles the sidebar collapsed state', async () => {
  render(<App />);
  expect(useSettingsStore.getState().sidebarCollapsed).toBe(false);
  act(() => modKeyDown('\\'));
  await waitFor(() => expect(useSettingsStore.getState().sidebarCollapsed).toBe(true));
});

test('Recent is scoped to the active workspace, hiding cross-workspace entries', async () => {
  render(<App />);
  await act(async () => {}); // settle startup hydration

  // A note opened under workspace A appears in Recent while A is active.
  act(() => {
    useSourcesStore.getState().setRoot('/wsA');
    useSourcesStore.getState().addRecent({ path: '/wsA/a.md', name: 'a.md' }, '/wsA');
  });
  expect(screen.getByRole('button', { name: 'a.md' })).toBeTruthy();

  // Switching to workspace B hides A's entry — so it can never be reopened under
  // B's grant (which would produce an error tab).
  act(() => useSourcesStore.getState().setRoot('/wsB'));
  expect(screen.queryByRole('button', { name: 'a.md' })).toBeNull();

  // Switching back to A resurfaces it, openable in place.
  act(() => useSourcesStore.getState().setRoot('/wsA'));
  expect(screen.getByRole('button', { name: 'a.md' })).toBeTruthy();
});

test('flushes the latest session synchronously when the window is going away', async () => {
  const writeSpy = vi.spyOn(sessionWriter, 'write');
  render(<App />);
  await act(async () => {}); // settle startup hydration so the flush listener attaches

  // Shape the session AFTER hydration; the 1s debounce has NOT fired yet.
  act(() => {
    useSourcesStore.getState().setRoot('/wsX');
    useTabsStore.getState().openTab({ path: '/wsX/note.md', name: 'note.md' });
  });
  writeSpy.mockClear();

  // The window going away must enqueue the CURRENT state immediately.
  act(() => window.dispatchEvent(new Event('beforeunload')));

  expect(writeSpy).toHaveBeenCalledTimes(1);
  const data = writeSpy.mock.calls[0]![0];
  expect(data.root).toBe('/wsX');
  expect(data.tabs.map((tab) => tab.docRef.path)).toContain('/wsX/note.md');
});
