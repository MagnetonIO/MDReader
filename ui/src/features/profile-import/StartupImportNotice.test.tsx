import { cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { setReaderBackend, type ReaderBackend } from '../../platform/backend';
import { tauriBackend } from '../../platform/tauri-backend';
import { StartupImportNotice } from './StartupImportNotice';

vi.mock('../../shell/persistence', () => ({ isTauri: () => true }));
let restore: (() => void) | undefined;
afterEach(() => { restore?.(); restore = undefined; cleanup(); });

function startup(selectedLibraryUnavailable: boolean): ReaderBackend {
  const nativeProfileStartup = async () => ({ status: 'ok' as const, data: {
    migrationRequired: false, usingImportedLibrary: false, returnController: false, selectedLibraryUnavailable,
  } });
  return { ...tauriBackend, profile: { ...tauriBackend.profile, nativeProfileStartup } };
}

test('an unopenable selected library is a dismissible notice, not a blocker', async () => {
  restore = setReaderBackend(startup(true));
  render(<StartupImportNotice onImport={vi.fn()} />);
  expect(await screen.findByText('Your imported library couldn’t be opened')).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Dismiss' }));
  expect(screen.queryByText('Your imported library couldn’t be opened')).toBeNull();
});

test('a normal startup shows no notice', async () => {
  restore = setReaderBackend(startup(false));
  const view = render(<StartupImportNotice onImport={vi.fn()} />);
  await Promise.resolve();
  expect(view.container.textContent).toBe('');
});
