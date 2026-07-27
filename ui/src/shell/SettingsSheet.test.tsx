import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';

import type {
  CloudDocumentQueryResponse,
  CloudFilterInput,
  CloudProvider,
  CloudSourcesSnapshot,
  DiagnosticInfo,
  DiagnosticLevel,
  LibrarySnapshot,
  LibrarySourceDto,
  PickedLibrarySource,
} from '../bindings';
import { SettingsCommandError, type SettingsBackend } from '../platform/settings.js';
import { useSettingsStore } from '../stores/settings.js';
import { SettingsSheet } from './SettingsSheet.js';

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

function cloudResponse(
  provider: CloudProvider,
  fields: string[],
  name = 'Research note.md',
  recordKey = 'provider-record-1',
): CloudDocumentQueryResponse {
  return {
    query_id: `query-${recordKey}`,
    provider,
    schema_version: 'mdreader.Document/v1',
    version_pins: {
      catalog_version: 'catalog-v5',
      schema_version: 'mdreader.Document/v1',
      schema_checksum: 'a'.repeat(64),
      mapping_version: 'mdreader.Document.cloud-mapping/v1',
      mapping_checksum: 'b'.repeat(64),
      connector_version: `${provider}/v1`,
      source_version: 'source-v1',
      source_checksum: 'c'.repeat(64),
      policy_version: 'mdreader.read-only/v1',
      policy_checksum: 'd'.repeat(64),
    },
    completeness: 'complete',
    sources: [
      {
        source_id: 'b'.repeat(64),
        outcome: {
          kind: 'ran',
          mode: 'virtual',
          scanned: 1,
          returned: 1,
          pushed: [],
          residual: [],
        },
      },
    ],
    scanned: 1,
    rows: [
      {
        source_id: 'b'.repeat(64),
        record_key: recordKey,
        fields: {
          path: recordKey,
          title: name,
          modifiedAt: 1_800_000_000_000,
          size: 2048,
          ...(fields.includes('body')
            ? {
                body: `# ${name}\n\nConnected source content.`,
              }
            : {}),
        },
        provenance: fields.map((field) => ({
          field,
          providers: [
            {
              provider,
              source_id: 'b'.repeat(64),
              record_key: recordKey,
              mapping_version: 'mdreader.Document.cloud-mapping/v1',
              observation: 'observed' as const,
              observed_at_ms: 1_800_000_000_000,
            },
          ],
        })),
      },
    ],
  };
}

function cloudBackend(
  initial: CloudSourcesSnapshot,
): SettingsBackend & {
  listCloudSources: () => Promise<CloudSourcesSnapshot>;
  connectCloudSource: (provider: CloudProvider) => Promise<CloudSourcesSnapshot>;
  setCloudSourceEnabled: (
    provider: CloudProvider,
    enabled: boolean,
  ) => Promise<CloudSourcesSnapshot>;
  disconnectCloudSource: (provider: CloudProvider) => Promise<CloudSourcesSnapshot>;
  queryCloudDocuments: (
    provider: CloudProvider,
    filters: CloudFilterInput[],
    fields: string[],
    requestId: string,
  ) => Promise<CloudDocumentQueryResponse>;
} {
  let snapshot = initial;
  return {
    ...backend(),
    listCloudSources: async () => snapshot,
    connectCloudSource: async (provider) => {
      snapshot = {
        ...snapshot,
        sources: snapshot.sources.map((source) =>
          source.provider === provider
            ? { ...source, connection: 'connected', enabled: true }
            : source,
        ),
      };
      return snapshot;
    },
    setCloudSourceEnabled: async (provider, enabled) => {
      snapshot = {
        ...snapshot,
        sources: snapshot.sources.map((source) =>
          source.provider === provider ? { ...source, enabled } : source,
        ),
      };
      return snapshot;
    },
    disconnectCloudSource: async (provider) => {
      snapshot = {
        ...snapshot,
        sources: snapshot.sources.map((source) =>
          source.provider === provider
            ? { ...source, connection: 'disconnected', enabled: false }
            : source,
        ),
      };
      return snapshot;
    },
    queryCloudDocuments: vi.fn(
      async (provider: CloudProvider, _filters: CloudFilterInput[], fields: string[]) =>
        cloudResponse(provider, fields),
    ),
    cancelCloudDocumentQuery: vi.fn(async () => true),
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
    expect(screen.getByRole('heading', { name: 'Library' })).toBeTruthy();
    expect(screen.getByRole('heading', { name: 'Diagnostics' })).toBeTruthy();
    expect(screen.getByText('Local Markdown folders', { selector: 'strong' })).toBeTruthy();
    expect(screen.getByText(/stay on this device/i)).toBeTruthy();
    expect(screen.queryByText(/Google Drive|Dropbox|Notion|connector/i)).toBeNull();

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

  it('pauses a connected cloud source without disconnecting its account', async () => {
    const api = cloudBackend({
      version: 1,
      sources: [
        {
          id: 'b'.repeat(64),
          provider: 'google-drive',
          label: 'Google Drive',
          connection: 'connected',
          enabled: true,
        },
        {
          id: '',
          provider: 'dropbox',
          label: 'Dropbox',
          connection: 'disconnected',
          enabled: false,
        },
      ],
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

    const includeDrive = await screen.findByRole('switch', {
      name: 'Include Google Drive in Library',
    });
    expect(includeDrive.getAttribute('aria-checked')).toBe('true');

    fireEvent.click(includeDrive);
    await screen.findByText('Paused');
    expect(screen.getByRole('button', { name: 'Disconnect Google Drive' })).toBeTruthy();

    fireEvent.click(screen.getByRole('button', { name: 'Disconnect Google Drive' }));
    await screen.findByRole('button', { name: 'Connect Google Drive' });
    expect(
      screen.queryByRole('switch', { name: 'Include Google Drive in Library' }),
    ).toBeNull();
  });

  it('connects Dropbox through the app account flow and includes it in the Library', async () => {
    const api = cloudBackend({
      version: 1,
      sources: [
        {
          id: '',
          provider: 'google-drive',
          label: 'Google Drive',
          connection: 'disconnected',
          enabled: false,
        },
        {
          id: '',
          provider: 'dropbox',
          label: 'Dropbox',
          connection: 'disconnected',
          enabled: false,
        },
      ],
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

    fireEvent.click(await screen.findByRole('button', { name: 'Connect Dropbox' }));
    const includeDropbox = await screen.findByRole('switch', {
      name: 'Include Dropbox in Library',
    });
    expect(includeDropbox.getAttribute('aria-checked')).toBe('true');
  });

  it('reads connected cloud documents through the native typed-query boundary', async () => {
    const api = cloudBackend({
      version: 1,
      sources: [
        {
          id: 'b'.repeat(64),
          provider: 'google-drive',
          label: 'Google Drive',
          connection: 'connected',
          enabled: true,
        },
        {
          id: '',
          provider: 'dropbox',
          label: 'Dropbox',
          connection: 'disconnected',
          enabled: false,
        },
      ],
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

    fireEvent.click(await screen.findByRole('button', { name: 'Browse Google Drive' }));
    expect(await screen.findByText('Research note.md')).toBeTruthy();
    expect(screen.getByLabelText('Google Drive documents').textContent).toContain('2 KiB');
    expect(api.queryCloudDocuments).toHaveBeenCalledWith(
      'google-drive',
      [],
      ['path', 'title', 'modifiedAt', 'size'],
      expect.stringMatching(/^browse:google-drive:/),
    );

    fireEvent.click(screen.getByRole('button', { name: /Research note\.md/ }));
    expect(
      (await screen.findByRole('region', { name: 'Research note.md preview' })).textContent,
    ).toContain('Connected source content.');
    expect(screen.getByRole('link', { name: 'Open original' }).getAttribute('href')).toBe(
      'https://drive.google.com/open?id=provider-record-1',
    );
    expect(screen.getByText('mdreader.Document.cloud-mapping/v1')).toBeTruthy();
  });

  it('does not publish a stale cloud browse completion after its replacement', async () => {
    let releaseFirst!: (response: CloudDocumentQueryResponse) => void;
    const first = new Promise<CloudDocumentQueryResponse>((resolve) => {
      releaseFirst = resolve;
    });
    const api = cloudBackend({
      version: 1,
      sources: [
        {
          id: 'b'.repeat(64),
          provider: 'google-drive',
          label: 'Google Drive',
          connection: 'connected',
          enabled: true,
        },
        {
          id: '',
          provider: 'dropbox',
          label: 'Dropbox',
          connection: 'disconnected',
          enabled: false,
        },
      ],
    });
    api.queryCloudDocuments = vi
      .fn<typeof api.queryCloudDocuments>()
      .mockImplementationOnce(() => first)
      .mockResolvedValueOnce(
        cloudResponse(
          'google-drive',
          ['path', 'title', 'modifiedAt', 'size'],
          'Current.md',
          'current',
        ),
      );

    render(
      <SettingsSheet
        backend={api}
        watcher="idle"
        importState={null}
        onActivateSource={vi.fn(async () => true)}
        onClose={vi.fn()}
      />,
    );

    const browse = await screen.findByRole('button', { name: 'Browse Google Drive' });
    fireEvent.click(browse);
    fireEvent.click(browse);
    expect(await screen.findByText('Current.md')).toBeTruthy();

    await act(async () => {
      releaseFirst(
        cloudResponse(
          'google-drive',
          ['path', 'title', 'modifiedAt', 'size'],
          'Retired.md',
          'retired',
        ),
      );
      await first;
    });

    expect(screen.getByText('Current.md')).toBeTruthy();
    expect(screen.queryByText('Retired.md')).toBeNull();
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

    await screen.findByText('Research notes');
    fireEvent.click(screen.getByRole('button', { name: 'Forget Research notes' }));

    expect(onSourceIntent).toHaveBeenCalledOnce();
    expect(forgetSource).toHaveBeenCalledWith(SOURCE.id);
    expect(onSourceIntent.mock.invocationCallOrder[0]).toBeLessThan(
      forgetSource.mock.invocationCallOrder[0]!,
    );

    await act(async () => {
      release({ version: 4, sources: [], cleanup: { kind: 'complete' } });
      await pending;
    });
  });

  it('keeps unavailable membership visible and protects the active source from forgetting', async () => {
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
        cleanup: { kind: 'complete' as const },
      })),
    });
    render(
      <SettingsSheet
        backend={api}
        watcher="watching"
        importState={null}
        onActivateSource={vi.fn(async () => true)}
        onClose={vi.fn()}
      />,
    );

    await screen.findByText('Archive');
    expect(screen.getByText('Folder not found')).toBeTruthy();
    expect(
      (screen.getByRole('button', { name: 'Forget Research notes' }) as HTMLButtonElement)
        .disabled,
    ).toBe(true);
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

    await screen.findByText('Research notes');
    fireEvent.click(screen.getByRole('button', { name: 'Forget Research notes' }));
    await screen.findByText('MDReader could not open that source.');
  });

  it('keeps a failed managed-state deletion visible as pending local cleanup', async () => {
    const api = backend({
      listSources: vi.fn(async () => ({
        version: 4,
        sources: [],
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

    const focusable = Array.from(
      dialog.querySelectorAll<HTMLElement>('button:not([disabled]), input:not([disabled])'),
    );
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
