import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';

import App from './App';
import { useSettingsStore } from './stores/settings';
import { useSourcesStore } from './stores/sources';
import { useTabsStore } from './stores/tabs';
import { setTestPlatform } from './test/tauri-os';

function installTauriRuntime(): void {
  Object.defineProperty(window, '__TAURI_INTERNALS__', {
    configurable: true,
    value: { invoke: vi.fn(() => new Promise(() => undefined)) },
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
    expect(
      screen.getByText('Unavailable until a durable mobile folder grant is implemented'),
    ).toBeTruthy();
    expect(screen.queryByRole('button', { name: 'Add folder' })).toBeNull();
    expect(screen.getByLabelText('Server URL')).toBeTruthy();
    expect(screen.getByLabelText('Session reference')).toBeTruthy();
  });

  test('Enterprise setup rejects embedded credentials and insecure remote origins', () => {
    render(<App />);
    fireEvent.click(screen.getByRole('button', { name: 'Sources' }));

    const url = screen.getByLabelText('Server URL');
    const session = screen.getByLabelText('Session reference');
    fireEvent.change(session, { target: { value: 'managed-session-42' } });

    fireEvent.change(url, { target: { value: 'https://admin:secret@catdb.example.com' } });
    fireEvent.click(screen.getByRole('button', { name: 'Use server' }));
    expect(screen.getByRole('alert').textContent).toMatch(/must not include credentials/i);

    fireEvent.change(url, { target: { value: 'http://catdb.example.com' } });
    fireEvent.click(screen.getByRole('button', { name: 'Use server' }));
    expect(screen.getByRole('alert').textContent).toMatch(/https/i);
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
    fireEvent.click(screen.getByRole('button', { name: 'Evidence' }));
    expect(screen.getByRole('main', { name: 'Semantic Inspector' })).toBeTruthy();
    expect(screen.queryByRole('navigation', { name: 'Primary' })).toBeNull();
    phone.unmount();
    useTabsStore.getState().reset();

    setViewport(900);
    render(<App />);
    fireEvent.click(screen.getByRole('button', { name: 'architecture.md' }));
    expect(screen.getByRole('main', { name: 'Reader' })).toBeTruthy();
    expect(screen.getByRole('region', { name: 'Library' })).toBeTruthy();
  });
});
