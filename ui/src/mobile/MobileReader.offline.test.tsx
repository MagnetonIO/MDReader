import { cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';

import type { TabState } from '../stores/types';
import { MobileReader } from './MobileReader';

const fixtures = vi.hoisted(() => ({
  readDoc: vi.fn(),
  cache: null as unknown,
}));

vi.mock('../platform/fs', () => ({
  readDoc: fixtures.readDoc,
}));

vi.mock('@tauri-apps/plugin-store', () => ({
  load: async () => ({
    get: async () => fixtures.cache,
    set: async () => undefined,
    save: async () => undefined,
  }),
}));

const tab: TabState = {
  id: 'tab-offline',
  docRef: { path: '/notes/cached.md', name: 'cached.md' },
  scroll: { block: 0, offset: 0 },
  history: [{ path: '/notes/cached.md', name: 'cached.md' }],
  historyIndex: 0,
  pinned: false,
  originRoot: '/notes',
};

function cachedEntry(overrides: Record<string, unknown> = {}) {
  const now = Date.now();
  return {
    version: 1,
    entries: [
      {
        source: { kind: 'device-folder', id: 'library-source-7' },
        sourceRoot: '/notes',
        documentPath: '/notes/cached.md',
        revision: 'blake3:abc123',
        observedAt: now - 2_000,
        cachedAt: now - 1_000,
        expiresAt: now + 60_000,
        content: '# Cached note\n\nRead while disconnected.',
        bytes: 39,
        ...overrides,
      },
    ],
  };
}

beforeEach(() => {
  fixtures.readDoc.mockRejectedValue(new Error('network unavailable'));
  fixtures.cache = cachedEntry();
});

afterEach(() => {
  cleanup();
  vi.clearAllMocks();
});

describe('mobile offline document fallback', () => {
  test('renders a source-attributed cached document without claiming library completeness', async () => {
    render(
      <MobileReader
        tab={tab}
        mode="reader"
        onBack={() => undefined}
        onModeChange={() => undefined}
      />,
    );

    expect(await screen.findByRole('heading', { name: 'Cached note' })).toBeTruthy();
    expect(screen.getByRole('status').textContent).toContain('Offline copy');
    expect(screen.getByRole('status').textContent).toContain('library-source-7');
    expect(screen.getByRole('status').textContent).toContain('blake3:abc123');
    expect(screen.getByRole('status').textContent).toMatch(/not a complete library/i);
  });

  test('keeps expired, corrupt, and missing cache outcomes distinct', async () => {
    fixtures.cache = cachedEntry({ expiresAt: Date.now() - 1 });
    const expired = render(
      <MobileReader
        tab={tab}
        mode="reader"
        onBack={() => undefined}
        onModeChange={() => undefined}
      />,
    );
    expect((await screen.findByRole('status')).textContent).toMatch(/offline copy expired/i);
    expired.unmount();

    fixtures.cache = cachedEntry({ content: 42 });
    const corrupt = render(
      <MobileReader
        tab={tab}
        mode="reader"
        onBack={() => undefined}
        onModeChange={() => undefined}
      />,
    );
    await waitFor(() =>
      expect(screen.getByRole('status').textContent).toMatch(/offline copy is corrupt/i),
    );
    corrupt.unmount();

    fixtures.cache = { version: 1, entries: [] };
    render(
      <MobileReader
        tab={tab}
        mode="reader"
        onBack={() => undefined}
        onModeChange={() => undefined}
      />,
    );
    await waitFor(() => expect(screen.getByRole('status').textContent).toMatch(/no offline copy/i));
  });
});
