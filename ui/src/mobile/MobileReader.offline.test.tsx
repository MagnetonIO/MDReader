import { cleanup, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, test, vi } from 'vitest';

import type { TabState } from '../stores/types';
import { MobileReader } from './MobileReader';
import type { MobileDocumentIdentity } from './offline-cache';

const fixtures = vi.hoisted(() => ({
  readDoc: vi.fn(),
  cache: null as unknown,
  parseMarkdown: vi.fn(),
}));

vi.mock('../platform/fs', () => ({
  readDoc: fixtures.readDoc,
}));

vi.mock('@mdreader/markdown/worker', async (importOriginal) => {
  const actual = await importOriginal<typeof import('@mdreader/markdown/worker')>();
  return {
    ...actual,
    parseMarkdownAuto: fixtures.parseMarkdown,
  };
});

vi.mock('@tauri-apps/plugin-store', () => ({
  load: async () => ({
    get: async () => fixtures.cache,
    set: async () => undefined,
    save: async () => undefined,
  }),
}));

const actualMarkdownWorker = await vi.importActual<typeof import('@mdreader/markdown/worker')>(
  '@mdreader/markdown/worker',
);

const tab: TabState = {
  id: 'tab-offline',
  docRef: { path: '/notes/cached.md', name: 'cached.md' },
  scroll: { block: 0, offset: 0 },
  history: [{ path: '/notes/cached.md', name: 'cached.md' }],
  historyIndex: 0,
  pinned: false,
  originRoot: '/notes',
};

const identity: MobileDocumentIdentity = {
  source: { kind: 'device-folder', id: 'library-source-7' },
  sourceRoot: { id: 'root-notes', version: 'grant-7' },
  document: { id: 'cached-note-1', path: '/notes/cached.md' },
};

function cachedEntry(overrides: Record<string, unknown> = {}) {
  const now = Date.now();
  return {
    version: 2,
    entries: [
      {
        source: { kind: 'device-folder', id: 'library-source-7' },
        sourceRoot: { id: 'root-notes', version: 'grant-7' },
        document: { id: 'cached-note-1', path: '/notes/cached.md' },
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
  fixtures.parseMarkdown.mockImplementation(actualMarkdownWorker.parseMarkdownAuto);
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
        sourceIdentity={identity}
        onBack={() => undefined}
        onModeChange={() => undefined}
      />,
    );

    expect(await screen.findByText('Cached note', { selector: 'h1' })).toBeTruthy();
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
        sourceIdentity={identity}
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
        sourceIdentity={identity}
        onBack={() => undefined}
        onModeChange={() => undefined}
      />,
    );
    await waitFor(() =>
      expect(screen.getByRole('status').textContent).toMatch(/offline copy is corrupt/i),
    );
    corrupt.unmount();

    fixtures.cache = { version: 2, entries: [] };
    render(
      <MobileReader
        tab={tab}
        mode="reader"
        sourceIdentity={identity}
        onBack={() => undefined}
        onModeChange={() => undefined}
      />,
    );
    await waitFor(() => expect(screen.getByRole('status').textContent).toMatch(/no offline copy/i));
  });

  test('rejects a cached body from a different admitted source identity', async () => {
    fixtures.cache = cachedEntry({
      source: { kind: 'device-folder', id: 'revoked-source' },
    });

    render(
      <MobileReader
        tab={tab}
        mode="reader"
        sourceIdentity={identity}
        onBack={() => undefined}
        onModeChange={() => undefined}
      />,
    );

    await waitFor(() => expect(screen.getByRole('status').textContent).toMatch(/no offline copy/i));
    expect(screen.queryByText('Cached note', { selector: 'h1' })).toBeNull();
  });

  test('rejects a live payload whose authority differs from the requested document', async () => {
    fixtures.cache = { version: 2, entries: [] };
    const loadDocument = vi.fn(async () => ({
      ...identity,
      source: { kind: 'device-folder' as const, id: 'replacement-source' },
      content: '# Wrong authority',
      revision: 'blake3:wrong',
      observedAt: Date.now(),
      provenance: null,
    }));

    render(
      <MobileReader
        tab={tab}
        mode="reader"
        sourceIdentity={identity}
        loadDocument={loadDocument}
        onBack={() => undefined}
        onModeChange={() => undefined}
      />,
    );

    await waitFor(() =>
      expect(screen.getByRole('status').textContent).toMatch(/identity changed/i),
    );
    expect(screen.queryByText('Wrong authority', { selector: 'h1' })).toBeNull();
  });

  test('a superseded cached parse cannot paint beneath the newly selected document', async () => {
    let releaseFirst: ((value: Awaited<ReturnType<typeof actualMarkdownWorker.parseMarkdownAuto>>) => void) | undefined;
    const firstParse = new Promise<Awaited<ReturnType<typeof actualMarkdownWorker.parseMarkdownAuto>>>(
      (resolve) => {
        releaseFirst = resolve;
      },
    );
    const firstParsed = await actualMarkdownWorker.parseMarkdownAuto('# First cached note');
    fixtures.parseMarkdown
      .mockImplementationOnce(() => firstParse)
      .mockImplementation(actualMarkdownWorker.parseMarkdownAuto);

    const now = Date.now();
    fixtures.cache = {
      version: 2,
      entries: [
        cachedEntry().entries[0],
        {
          ...cachedEntry().entries[0],
          document: { id: 'cached-note-2', path: '/notes/second.md' },
          revision: 'blake3:second',
          content: '# Second cached note',
          bytes: 20,
          cachedAt: now,
        },
      ],
    };
    const secondTab: TabState = {
      ...tab,
      id: 'tab-second',
      docRef: { path: '/notes/second.md', name: 'second.md' },
      history: [{ path: '/notes/second.md', name: 'second.md' }],
    };
    const secondIdentity: MobileDocumentIdentity = {
      ...identity,
      document: { id: 'cached-note-2', path: '/notes/second.md' },
    };

    const view = render(
      <MobileReader
        tab={tab}
        mode="reader"
        sourceIdentity={identity}
        onBack={() => undefined}
        onModeChange={() => undefined}
      />,
    );
    await waitFor(() => expect(fixtures.parseMarkdown).toHaveBeenCalledTimes(1));

    view.rerender(
      <MobileReader
        tab={secondTab}
        mode="reader"
        sourceIdentity={secondIdentity}
        onBack={() => undefined}
        onModeChange={() => undefined}
      />,
    );
    expect(await screen.findByText('Second cached note', { selector: 'h1' })).toBeTruthy();

    releaseFirst?.(firstParsed);
    await waitFor(() => expect(screen.queryByText('First cached note')).toBeNull());
    expect(screen.getByText('Second cached note', { selector: 'h1' })).toBeTruthy();
  });

  test('a cached parse failure reaches a terminal typed error instead of loading forever', async () => {
    fixtures.parseMarkdown.mockRejectedValueOnce(new Error('parse failed'));

    render(
      <MobileReader
        tab={tab}
        mode="reader"
        sourceIdentity={identity}
        onBack={() => undefined}
        onModeChange={() => undefined}
      />,
    );

    await waitFor(() =>
      expect(screen.getByRole('status').textContent).toMatch(/offline copy is corrupt/i),
    );
    expect(screen.queryByText(/loading document/i)).toBeNull();
  });
});
