import {
  act,
  cleanup,
  fireEvent,
  render,
  screen,
  waitFor,
  within,
} from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type {
  DiagnosticInfo,
  DiagnosticLevel,
  LibrarySnapshot,
  LibrarySourceDto,
  PickedLibrarySource,
} from '../bindings';
import { SettingsCommandError, type SettingsBackend } from '../platform/settings.js';
import { useSettingsStore } from '../stores/settings.js';
import { visibleSettingsControls } from '../features/settings/focus.js';
import {
  SettingsSheet as RequiredSettingsSheet,
  type SettingsSheetProps,
} from './SettingsSheet.js';

function SettingsSheet(
  props: Omit<SettingsSheetProps, 'onForgetActiveSource'> &
    Partial<Pick<SettingsSheetProps, 'onForgetActiveSource'>>,
) {
  return (
    <RequiredSettingsSheet
      {...props}
      onForgetActiveSource={props.onForgetActiveSource ?? (async () => {})}
    />
  );
}

function selectSettingsSection(name: string) {
  fireEvent.click(screen.getByRole('button', { name }));
}

const SOURCE_ID = 'a'.repeat(64);
const SOURCE: LibrarySourceDto = {
  id: SOURCE_ID,
  name: 'Research notes',
  path: '/Users/alex/Notes',
  state: { kind: 'available' },
  index_state: 'present',
  active: false,
  restore_candidate: false,
};

const SNAPSHOT: LibrarySnapshot = {
  version: 4,
  sources: [SOURCE],
  cloud_sources: [],
  connected_catdb: null,
  connected_catdb_rediscovery_required: false,
  cleanup: { kind: 'complete' },
};
const DIAGNOSTICS: DiagnosticInfo = {
  level: 'standard',
  max_file_bytes: 256 * 1024,
  max_retained_files: 3,
  max_export_bytes: 512 * 1024,
};

function backend(overrides: Partial<SettingsBackend> = {}): SettingsBackend {
  return {
    listSources: vi.fn(async () => SNAPSHOT),
    pickAndAddSource: vi.fn(
      async (): Promise<PickedLibrarySource> => ({ source: SOURCE, added: true }),
    ),
    reconcileRestore: vi.fn(async () => null),
    markActive: vi.fn(async () => SNAPSHOT),
    forgetSource: vi.fn(async () => ({
      version: 4,
      sources: [],
      cloud_sources: [],
      connected_catdb: null,
      connected_catdb_rediscovery_required: false,
      cleanup: { kind: 'complete' as const },
    })),
    diagnosticsInfo: vi.fn(async () => DIAGNOSTICS),
    setDiagnosticLevel: vi.fn(async (level: DiagnosticLevel) => ({
      ...DIAGNOSTICS,
      level,
    })),
    recordDiagnosticEvent: vi.fn(async () => {}),
    revealDiagnostics: vi.fn(async () => {}),
    exportDiagnostics: vi.fn(async () => ({
      status: 'exported' as const,
      bytes_written: 128,
    })),
    clearDiagnostics: vi.fn(async () => DIAGNOSTICS),
    ...overrides,
  };
}

beforeEach(() => {
  useSettingsStore.getState().reset();
});

afterEach(() => {
  cleanup();
  useSettingsStore.getState().reset();
});

describe('Settings sheet', () => {
  it('keeps semantic headings and theme choices without decorative product or ordinal markers', () => {
    render(
      <SettingsSheet
        backend={backend()}
        watcher="idle"
        importState={null}
        onActivateSource={vi.fn(async () => true)}
        onClose={vi.fn()}
      />,
    );

    const dialog = screen.getByRole('dialog', { name: 'Settings' });
    expect(within(dialog).getByRole('heading', { name: 'Settings', level: 1 })).toBeTruthy();
    expect(within(dialog).getByRole('heading', { name: 'Appearance', level: 2 })).toBeTruthy();
    selectSettingsSection('Library & sources');
    expect(within(dialog).getByRole('heading', { name: 'Library & sources', level: 2 })).toBeTruthy();
    selectSettingsSection('Advanced');
    expect(within(dialog).getByRole('heading', { name: 'Diagnostics', level: 2 })).toBeTruthy();
    selectSettingsSection('Appearance');
    expect(within(dialog).getByRole('button', { name: 'System' })).toBeTruthy();
    expect(within(dialog).getByRole('button', { name: 'Light' })).toBeTruthy();
    expect(within(dialog).getByRole('button', { name: 'Dark' })).toBeTruthy();
    expect(within(dialog).queryByText('MDReader', { exact: true })).toBeNull();
    expect(screen.queryByText('01')).toBeNull();
    expect(screen.queryByText('02')).toBeNull();
    expect(within(dialog).queryByText('03', { exact: true })).toBeNull();
  });

  it('uses the existing appearance store and presents the local-only product boundary', async () => {
    const api = backend();
    render(
      <SettingsSheet
        backend={api}
        watcher="watching"
        importState={null}
        onActivateSource={vi.fn(async () => true)}
        onClose={vi.fn()}
      />,
    );

    expect(screen.getByRole('dialog', { name: 'Settings' })).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'Appearance' })).toBeTruthy();
    selectSettingsSection('Library & sources');
    expect(screen.getByRole('heading', { name: 'Library & sources' })).toBeTruthy();
    selectSettingsSection('Advanced');
    expect(screen.getByRole('heading', { name: 'Diagnostics' })).toBeTruthy();
    expect(screen.getByText('Local Markdown folders', { selector: 'strong' })).toBeTruthy();
    expect(screen.getByText(/Coarse lifecycle logs stay on this device/i)).toBeTruthy();
    expect(screen.queryByText(/Google Drive|Dropbox|Notion|connector/i)).toBeNull();

    selectSettingsSection('Appearance');
    fireEvent.click(screen.getByRole('button', { name: 'Dark' }));
    expect(useSettingsStore.getState().theme).toBe('dark');
    expect(document.documentElement.getAttribute('data-theme')).not.toBe('dark');

    fireEvent.click(screen.getByRole('button', { name: 'Sans' }));
    expect(useSettingsStore.getState().readingFont).toBe('sans');
    fireEvent.click(screen.getByRole('button', { name: 'Increase text size' }));
    expect(useSettingsStore.getState().textScaleIndex).toBe(4);

    await screen.findByText('Research notes');
    expect(api.listSources).toHaveBeenCalledOnce();
    expect(api.diagnosticsInfo).toHaveBeenCalledOnce();
  });

  it('keeps developer migration commands out of Settings', async () => {
    const api = backend();
    render(<SettingsSheet backend={api} watcher="idle" importState={null}
      onActivateSource={async () => true} onClose={() => {}} initialSection="advanced" />);
    expect(screen.queryByText('Preserve an offline snapshot')).toBeNull();
    expect(screen.queryByText('Migration & archived records')).toBeNull();
    expect(screen.queryByLabelText('Offline archive commands')).toBeNull();
    fireEvent.click(screen.getByRole('button', { name: 'Backup & storage' }));
    expect(screen.queryByText(/Advanced → Migration/)).toBeNull();
    expect(screen.queryByRole('textbox', { name: /bearer|origin/i })).toBeNull();
    expect(screen.queryByRole('button', { name: /Connect Dropbox|Reconnect|Enroll/i })).toBeNull();
    expect(api.listSources).toHaveBeenCalledOnce();
  });

  it('activates a persisted member through the shell-owned switch callback', async () => {
    const api = backend();
    const activate = vi.fn(async () => false);
    const close = vi.fn();
    render(
      <SettingsSheet
        backend={api}
        watcher="idle"
        importState={null}
        onActivateSource={activate}
        onClose={close}
      />,
    );
    selectSettingsSection('Library & sources');

    await screen.findByText('Research notes');
    await act(async () => {
      fireEvent.click(screen.getByRole('button', { name: 'Activate Research notes' }));
    });

    expect(close).toHaveBeenCalledOnce();
    expect(activate).toHaveBeenCalledWith(SOURCE);
  });

  it('advances source intent synchronously before forgetting can await the backend', async () => {
    let release!: (snapshot: LibrarySnapshot) => void;
    const pending = new Promise<LibrarySnapshot>((resolve) => {
      release = resolve;
    });
    const forgetSource = vi.fn(() => pending);
    const api = backend({ forgetSource });
    const onSourceIntent = vi.fn();
    render(
      <SettingsSheet
        backend={api}
        watcher="idle"
        importState={null}
        onActivateSource={vi.fn(async () => true)}
        onSourceIntent={onSourceIntent}
        onClose={vi.fn()}
      />,
    );
    selectSettingsSection('Library & sources');

    await screen.findByText('Research notes');
    fireEvent.click(screen.getByRole('button', { name: 'Forget Research notes' }));

    expect(onSourceIntent).toHaveBeenCalledOnce();
    expect(forgetSource).toHaveBeenCalledWith(SOURCE.id);
    expect(onSourceIntent.mock.invocationCallOrder[0]).toBeLessThan(
      forgetSource.mock.invocationCallOrder[0]!,
    );

    await act(async () => {
      release({
        version: 4,
        sources: [],
        cloud_sources: [],
        connected_catdb: null,
        connected_catdb_rediscovery_required: false,
        cleanup: { kind: 'complete' },
      });
      await pending;
    });
  });

  it('keeps unavailable membership visible and lets the app own forgetting the active source', async () => {
    const active: LibrarySourceDto = {
      ...SOURCE,
      active: true,
      restore_candidate: true,
    };
    const missing: LibrarySourceDto = {
      ...SOURCE,
      id: 'b'.repeat(64),
      name: 'Archive',
      path: '/Volumes/Archive',
      state: { kind: 'missing' },
      index_state: 'not-created',
    };
    const api = backend({
      listSources: vi.fn(async () => ({
        version: 4,
        sources: [active, missing],
        cloud_sources: [],
        connected_catdb: null,
        connected_catdb_rediscovery_required: false,
        cleanup: { kind: 'complete' as const },
      })),
    });
    const onForgetActiveSource = vi.fn(async () => {});
    const onClose = vi.fn();
    render(
      <SettingsSheet
        backend={api}
        watcher="watching"
        importState={null}
        onActivateSource={vi.fn(async () => true)}
        onForgetActiveSource={onForgetActiveSource}
        onClose={onClose}
      />,
    );
    selectSettingsSection('Library & sources');

    await screen.findByText('Archive');
    expect(screen.getByText('Folder not found')).toBeTruthy();
    const forgetActive = screen.getByRole('button', { name: 'Forget Research notes' });
    expect((forgetActive as HTMLButtonElement).disabled).toBe(false);
    fireEvent.click(forgetActive);
    const confirm = screen.getByRole('alertdialog', { name: 'Forget Research notes' });
    const cancel = within(confirm).getByRole('button', { name: 'Cancel' });
    const submit = within(confirm).getByRole('button', { name: 'Forget source' });
    expect(document.activeElement).toBe(cancel);
    submit.focus();
    fireEvent.keyDown(confirm, { key: 'Tab' });
    expect(document.activeElement).toBe(cancel);
    fireEvent.keyDown(confirm, { key: 'Escape' });
    expect(screen.queryByRole('alertdialog', { name: 'Forget Research notes' })).toBeNull();
    expect(onClose).not.toHaveBeenCalled();
    expect(document.activeElement).toBe(forgetActive);

    fireEvent.click(forgetActive);
    fireEvent.click(screen.getByRole('button', { name: 'Forget source' }));
    expect(onForgetActiveSource).toHaveBeenCalledWith(active);
    expect(api.forgetSource).not.toHaveBeenCalled();
    expect(
      (screen.getByRole('button', { name: 'Activate Archive' }) as HTMLButtonElement).disabled,
    ).toBe(true);
    expect(screen.getByText(/Index on disk/i)).toBeTruthy();
  });

  it('requires a second explicit confirmation before clearing the bounded local logs', async () => {
    const api = backend();
    render(
      <SettingsSheet
        backend={api}
        watcher="idle"
        importState={null}
        onActivateSource={vi.fn(async () => true)}
        onClose={vi.fn()}
      />,
    );
    selectSettingsSection('Advanced');

    await screen.findByText(/256 KiB per file/i);
    fireEvent.click(screen.getByRole('button', { name: 'Clear logs' }));
    expect(api.clearDiagnostics).not.toHaveBeenCalled();

    const confirmation = screen.getByRole('alertdialog', { name: 'Clear diagnostic logs' });
    expect(confirmation.textContent).toMatch(/cannot be undone/i);
    const cancel = screen.getByRole('button', { name: 'Cancel' });
    const clearNow = screen.getByRole('button', { name: 'Clear diagnostic logs now' });
    await waitFor(() => expect(document.activeElement).toBe(cancel));
    fireEvent.keyDown(confirmation, { key: 'Tab', shiftKey: true });
    expect(document.activeElement).toBe(clearNow);
    await act(async () => {
      fireEvent.click(clearNow);
    });
    expect(api.clearDiagnostics).toHaveBeenCalledWith(true);
  });

  it('reports retention as a maximum without exposing a native diagnostics path', async () => {
    render(
      <SettingsSheet
        backend={backend()}
        watcher="idle"
        importState={null}
        onActivateSource={vi.fn(async () => true)}
        onClose={vi.fn()}
      />,
    );
    selectSettingsSection('Advanced');

    await screen.findByText(/up to 3 files/i);
    expect(screen.queryByText('/Users/alex/Library/Logs/MDReader')).toBeNull();
    expect(screen.getByRole('button', { name: 'Reveal' })).toBeTruthy();
  });

  it('uses local copy for the closed backend failure code', async () => {
    const api = backend({
      forgetSource: vi.fn(async () => {
        throw new SettingsCommandError('library', {
          kind: 'open-failed',
        });
      }),
    });
    render(
      <SettingsSheet
        backend={api}
        watcher="idle"
        importState={null}
        onActivateSource={vi.fn(async () => true)}
        onClose={vi.fn()}
      />,
    );
    selectSettingsSection('Library & sources');

    await screen.findByText('Research notes');
    fireEvent.click(screen.getByRole('button', { name: 'Forget Research notes' }));
    await screen.findByText('MDReader could not open that source.');
  });

  it('keeps a failed managed-state deletion visible as pending local cleanup', async () => {
    const api = backend({
      listSources: vi.fn(async () => ({
        version: 4,
        sources: [],
        cloud_sources: [],
        connected_catdb: null,
        connected_catdb_rediscovery_required: false,
        cleanup: { kind: 'pending' as const, source_count: 2 },
      })),
    });
    render(
      <SettingsSheet
        backend={api}
        watcher="idle"
        importState={null}
        onActivateSource={vi.fn(async () => true)}
        onClose={vi.fn()}
      />,
    );
    selectSettingsSection('Library & sources');

    await screen.findByText(/finishing local cleanup for 2 sources/i);
  });

  it.each([0, 1, 3])('renders an exact retention maximum of %i without fabricating a live count', async (maximum) => {
    const api = backend({
      diagnosticsInfo: vi.fn(async () => ({ ...DIAGNOSTICS, max_retained_files: maximum })),
    });
    render(
      <SettingsSheet
        backend={api}
        watcher="idle"
        importState={null}
        onActivateSource={vi.fn(async () => true)}
        onClose={vi.fn()}
      />,
    );
    selectSettingsSection('Advanced');

    await screen.findByText(new RegExp(`up to ${maximum} ${maximum === 1 ? 'file' : 'files'}`, 'i'));
  });

  it('traps keyboard focus, closes on Escape, and restores the invoking focus', async () => {
    const trigger = document.createElement('button');
    trigger.textContent = 'Open settings';
    document.body.append(trigger);
    trigger.focus();
    const close = vi.fn();

    const { unmount } = render(
      <SettingsSheet
        backend={backend()}
        watcher="idle"
        importState={null}
        onActivateSource={vi.fn(async () => true)}
        onClose={close}
      />,
    );
    const dialog = screen.getByRole('dialog', { name: 'Settings' });
    await waitFor(() => expect(dialog.contains(document.activeElement)).toBe(true));

    const focusable = visibleSettingsControls(dialog);
    focusable.at(-1)!.focus();
    fireEvent.keyDown(dialog, { key: 'Tab' });
    expect(document.activeElement).toBe(focusable[0]);

    fireEvent.keyDown(dialog, { key: 'Escape' });
    expect(close).toHaveBeenCalledOnce();
    unmount();
    expect(document.activeElement).toBe(trigger);
    trigger.remove();
  });
});
