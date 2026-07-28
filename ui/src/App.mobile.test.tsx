import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';

import App from './App';
import { useSettingsStore } from './stores/settings';
import { setTestPlatform } from './test/tauri-os';

const persistence = vi.hoisted(() => ({
  stores: new Map<string, Map<string, unknown>>(),
}));
let nativeEnterpriseConfigured = false;

vi.mock('@tauri-apps/plugin-store', () => ({
  load: async (name: string) => ({
      get: async (key: string) => persistence.stores.get(name)?.get(key) ?? null,
      set: async (key: string, value: unknown) => {
        let store = persistence.stores.get(name);
        if (!store) {
          store = new Map();
          persistence.stores.set(name, store);
        }
        store.set(key, value);
      },
      save: async () => undefined,
    }),
}));

function installTauriRuntime(): void {
  Object.defineProperty(window, '__TAURI_INTERNALS__', {
    configurable: true,
    value: {
      invoke: vi.fn(
        async (
          command: string,
          args?: { operation?: string; bodyJson?: string | null },
        ) => {
          if (command === 'mobile_enterprise_request') {
            const path =
              args?.operation === 'session' ? '/v1/session' : '/v1/entity/query';
            const response = await fetch(`https://catdb.example.com${path}`, {
              method: args?.operation === 'session' ? 'GET' : 'POST',
              body: args?.bodyJson ?? undefined,
            });
            return { status: response.status, body_json: await response.text() };
          }
          if (command === 'mobile_enterprise_connection') {
            return nativeEnterpriseConfigured
              ? {
                  connection_id: 'managed-connection-42',
                  label: 'Enterprise test connection',
                }
              : null;
          }
          return new Promise(() => undefined);
        },
      ),
    },
  });
}

let viewportWidth = 390;
let systemDark = false;
const mediaListeners = new Map<string, Set<(event: MediaQueryListEvent) => void>>();

function matches(query: string): boolean {
  if (query === '(min-width: 768px)') return viewportWidth >= 768;
  if (query === '(prefers-color-scheme: dark)') return systemDark;
  return false;
}

function installMatchMedia(): void {
  Object.defineProperty(window, 'matchMedia', {
    configurable: true,
    value: vi.fn((query: string) => {
      const listeners = mediaListeners.get(query) ?? new Set();
      mediaListeners.set(query, listeners);
      return {
        get matches() {
          return matches(query);
        },
        media: query,
        onchange: null,
        addEventListener: (
          _type: string,
          listener: EventListenerOrEventListenerObject,
        ) => {
          if (typeof listener === 'function') {
            listeners.add(listener as (event: MediaQueryListEvent) => void);
          }
        },
        removeEventListener: (
          _type: string,
          listener: EventListenerOrEventListenerObject,
        ) => {
          if (typeof listener === 'function') {
            listeners.delete(listener as (event: MediaQueryListEvent) => void);
          }
        },
        addListener: (listener: (event: MediaQueryListEvent) => void) => listeners.add(listener),
        removeListener: (listener: (event: MediaQueryListEvent) => void) =>
          listeners.delete(listener),
        dispatchEvent: () => true,
      } as MediaQueryList;
    }),
  });
}

function setViewport(width: number): void {
  viewportWidth = width;
  Object.defineProperty(window, 'innerWidth', {
    configurable: true,
    value: width,
  });
  for (const listener of mediaListeners.get('(min-width: 768px)') ?? []) {
    listener({ matches: matches('(min-width: 768px)'), media: '(min-width: 768px)' } as MediaQueryListEvent);
  }
}

function setSystemDark(dark: boolean): void {
  systemDark = dark;
  for (const listener of mediaListeners.get('(prefers-color-scheme: dark)') ?? []) {
    listener({
      matches: dark,
      media: '(prefers-color-scheme: dark)',
    } as MediaQueryListEvent);
  }
}

function settingsStore(): Map<string, unknown> {
  let store = persistence.stores.get('settings.json');
  if (!store) {
    store = new Map();
    persistence.stores.set('settings.json', store);
  }
  return store;
}

function installEnterpriseLibrary(): void {
  nativeEnterpriseConfigured = true;
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = String(input);
      if (url.endsWith('/v1/session')) {
        return new Response(
          JSON.stringify({
            deployment_id: 'deployment-1',
            principal_kind: 'oidc',
            scopes: ['query'],
            workspace_ids: ['research-root'],
          }),
          { status: 200 },
        );
      }
      if (url.endsWith('/v1/entity/query')) {
        const request = JSON.parse(String(init?.body)) as { select: string[] };
        const fields: Record<string, unknown> = {
          path: 'papers/architecture.md',
          title: 'Semantic architecture',
          size: 128,
        };
        if (request.select.includes('body')) {
          fields.body = '# Semantic architecture\n\nProvenance-aware reading.';
        }
        const provenance = Object.fromEntries(
          Object.entries(fields).map(([field, value]) => [
            field,
            {
              field,
              provenance_rule_version_id: 'provenance-rule-1',
              selected: 0,
              considered: [
                {
                  source_version_id: 'enterprise-source-version-1',
                  record_key: 'architecture-doc',
                  mapping_version_id: 'mapping-4',
                  observed_at_ms: 1_234,
                  state: { state: 'observed_value', value },
                },
              ],
            },
          ]),
        );
        return new Response(
          JSON.stringify({
            query_id: 'query-1',
            pins: {
              catalog_resolver_id: 'resolver-1',
              catalog_revision_id: 'catalog-17',
              schema_version_id: 'schema-9',
              physical_schema_version_ids: ['physical-schema-3'],
              mapping_version_ids: ['mapping-4'],
              source_version_ids: ['enterprise-source-version-1'],
              policy_version_ids: ['policy-2'],
              provenance_rule_version_id: 'provenance-rule-1',
            },
            rows: [{ record_key: 'architecture-doc', fields, provenance }],
            sources: [
              {
                source_id: 'enterprise-source-1',
                source_version_id: 'enterprise-source-version-1',
                scanned: 1,
                returned: 1,
                freshness_ms: 0,
                outcome: 'ran',
              },
            ],
            completeness: { state: 'complete' },
          }),
          { status: 200 },
        );
      }
      return new Response(null, { status: 404 });
    }),
  );
}

beforeEach(() => {
  installTauriRuntime();
  persistence.stores.clear();
  nativeEnterpriseConfigured = false;
  mediaListeners.clear();
  viewportWidth = 390;
  systemDark = false;
  installMatchMedia();
  setViewport(390);
  setTestPlatform('ios');
});

afterEach(() => {
  cleanup();
  vi.unstubAllGlobals();
  Reflect.deleteProperty(window, '__TAURI_INTERNALS__');
  useSettingsStore.getState().reset();
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
    expect(
      screen.getByText('Unavailable until native secure bearer enrollment is implemented.'),
    ).toBeTruthy();
    expect(screen.queryByLabelText('Native connection')).toBeNull();
    expect(screen.queryByLabelText('Server URL')).toBeNull();
    expect(screen.queryByLabelText('Session reference')).toBeNull();
  });

  test('hydrates and persists System, Light, and Dark choices across mobile relaunch', async () => {
    settingsStore().set('settings', {
      theme: 'dark',
      readingFont: 'serif',
      textScaleIndex: 3,
      sidebarCollapsed: false,
      outlineVisible: true,
      relatedRailOpen: false,
      inspectorOpen: false,
      lensSchema: null,
      reviewer: '',
    });
    render(<App />);
    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    await waitFor(() =>
      expect((screen.getByRole('radio', { name: 'Dark' }) as HTMLInputElement).checked).toBe(true),
    );
    expect(document.documentElement.dataset.theme).toBe('dark');

    fireEvent.click(screen.getByRole('radio', { name: 'Light' }));
    await waitFor(() =>
      expect(settingsStore().get('settings')).toEqual(
        expect.objectContaining({ theme: 'light' }),
      ),
    );

    cleanup();
    useSettingsStore.getState().reset();
    render(<App />);
    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    await waitFor(() =>
      expect((screen.getByRole('radio', { name: 'Light' }) as HTMLInputElement).checked).toBe(true),
    );
    expect(document.documentElement.dataset.theme).toBe('light');
    expect(screen.getByRole('radio', { name: 'System' })).toBeTruthy();
  });

  test('System follows a dark OS without converting the persisted choice to explicit Dark', async () => {
    setSystemDark(true);
    render(<App />);

    fireEvent.click(screen.getByRole('button', { name: 'Settings' }));
    await waitFor(() =>
      expect((screen.getByRole('radio', { name: 'System' }) as HTMLInputElement).checked).toBe(true),
    );
    expect(document.documentElement.hasAttribute('data-theme')).toBe(false);
    expect(document.querySelector('.mobile-shell')?.getAttribute('data-system-theme')).toBe('dark');
  });

  test('hydrates the saved Enterprise source into Library, title search, Reader, and Inspector', async () => {
    installEnterpriseLibrary();
    render(<App />);

    expect(await screen.findByRole('button', { name: 'architecture.md' })).toBeTruthy();
    fireEvent.click(screen.getByRole('button', { name: 'Search' }));
    fireEvent.change(screen.getByRole('searchbox', { name: 'Search' }), {
      target: { value: 'Semantic architecture' },
    });
    fireEvent.click(screen.getByRole('button', { name: 'architecture.md' }));
    expect(await screen.findByText('Semantic architecture', { selector: 'h1' })).toBeTruthy();
    expect(screen.getByRole('main', { name: 'Reader' })).toBeTruthy();
    expect(screen.queryByRole('region', { name: 'Library' })).toBeNull();

    fireEvent.click(screen.getByRole('button', { name: 'Evidence' }));
    expect(screen.getByRole('main', { name: 'Semantic Inspector' })).toBeTruthy();
    expect(screen.getByText('deployment-1')).toBeTruthy();
    expect(screen.getByText('schema-9')).toBeTruthy();
    expect(screen.getByText('mapping-4')).toBeTruthy();
    expect(screen.getByText('policy-2')).toBeTruthy();
    expect(screen.queryByRole('navigation', { name: 'Primary' })).toBeNull();
  });

  test('reacts to phone/tablet viewport changes in both directions while reading', async () => {
    installEnterpriseLibrary();
    render(<App />);
    fireEvent.click(await screen.findByRole('button', { name: 'architecture.md' }));
    expect(await screen.findByRole('main', { name: 'Reader' })).toBeTruthy();
    expect(screen.queryByRole('region', { name: 'Library' })).toBeNull();

    setViewport(900);
    await waitFor(() => expect(screen.getByRole('region', { name: 'Library' })).toBeTruthy());

    setViewport(390);
    await waitFor(() => expect(screen.queryByRole('region', { name: 'Library' })).toBeNull());
  });
});
