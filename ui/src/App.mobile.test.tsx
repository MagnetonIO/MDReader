import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, test } from 'vitest';

import { setTestPlatform } from '@tauri-apps/plugin-os';

import App from './App';
import { useSettingsStore } from './stores/settings';
import { useSourcesStore } from './stores/sources';
import { useTabsStore } from './stores/tabs';

function installTauriRuntime(): void {
  Object.defineProperty(window, '__TAURI_INTERNALS__', {
    configurable: true,
    value: {},
  });
}

function setViewport(width: number): void {
  Object.defineProperty(window, 'innerWidth', {
    configurable: true,
    value: width,
  });
}

beforeEach(() => {
  installTauriRuntime();
  setViewport(390);
  setTestPlatform('ios');
});

afterEach(() => {
  cleanup();
  Reflect.deleteProperty(window, '__TAURI_INTERNALS__');
  useSettingsStore.getState().reset();
  useSourcesStore.getState().reset();
  useTabsStore.getState().reset();
  setTestPlatform('macos');
});

describe('compile-time mobile composition', () => {
  test.each(['ios', 'android'] as const)(
    '%s mounts the phone tabs without mounting desktop chrome',
    (runtime) => {
      setTestPlatform(runtime);
      render(<App />);

      const navigation = screen.getByRole('navigation', { name: 'Primary' });
      expect(navigation.textContent).toBe('LibrarySearchSourcesSettings');
      expect(screen.queryByText('No open documents')).toBeNull();
      expect(screen.queryByText('Source')).toBeNull();
      expect(screen.queryByText('Split')).toBeNull();
      expect(screen.queryByRole('button', { name: /check for updates/i })).toBeNull();
    },
  );

  test('Sources exposes only the mobile allowlist and no direct credential form', () => {
    render(<App />);
    fireEvent.click(screen.getByRole('button', { name: 'Sources' }));

    for (const source of [
      'Device folders',
      'Google Drive',
      'Dropbox',
      'Enterprise CatDB server',
    ]) {
      expect(screen.getByRole('heading', { name: source })).toBeTruthy();
    }
    for (const forbidden of ['Database', 'Warehouse', 'Host', 'Port', 'Username', 'Password']) {
      expect(screen.queryByLabelText(forbidden)).toBeNull();
    }
    expect(screen.getAllByText('Unavailable on mobile until secure sign-in is supported')).toHaveLength(2);
    expect(screen.getByLabelText('Server URL')).toBeTruthy();
    expect(screen.getByLabelText('Session reference')).toBeTruthy();
  });

  test('Settings preserves the System, Light, and Dark native theme choices', () => {
    render(<App />);
    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));

    fireEvent.click(screen.getByRole('radio', { name: 'Dark' }));

    expect(useSettingsStore.getState().theme).toBe('dark');
    expect(document.documentElement.dataset.theme).toBe('dark');
    expect(screen.getByRole('radio', { name: 'System' })).toBeTruthy();
    expect(screen.getByRole('radio', { name: 'Light' })).toBeTruthy();
  });

  test('a phone opens a document as a full-screen reader while a tablet keeps a bounded library pane', () => {
    useSourcesStore.getState().setRoot('/notes');
    useSourcesStore.getState().setFiles([
      {
        path: '/notes/architecture.md',
        name: 'architecture.md',
        dir: '/notes',
        mtime: 1,
        size: 10,
        ignored: false,
      },
    ]);

    const phone = render(<App />);
    fireEvent.click(screen.getByRole('button', { name: 'architecture.md' }));
    expect(screen.getByRole('main', { name: 'Reader' })).toBeTruthy();
    expect(screen.queryByRole('region', { name: 'Library' })).toBeNull();
    phone.unmount();
    useTabsStore.getState().reset();

    setViewport(900);
    render(<App />);
    fireEvent.click(screen.getByRole('button', { name: 'architecture.md' }));
    expect(screen.getByRole('main', { name: 'Reader' })).toBeTruthy();
    expect(screen.getByRole('region', { name: 'Library' })).toBeTruthy();
  });
});
