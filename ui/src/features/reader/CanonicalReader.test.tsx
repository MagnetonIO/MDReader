import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { CanonicalReader } from './CanonicalReader';
import type { CanonicalReaderBackend } from './contracts';
import type { ProviderReaderReference } from '../../platform/providers';
import { useSettingsStore } from '../../stores/settings';

afterEach(() => { cleanup(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });
test('local attachments preserve the mounted reader, scroll and exact reference on Return', async () => {
  const reference = { sourceId: 'a'.repeat(64), path: 'index.md', revision: 'b'.repeat(64) };
  const backend: CanonicalReaderBackend = {
    readDocument: vi.fn(async () => ({ operation: 'read_document' as const, ...reference,
      markdown: '# Origin\n\n[SQL](assets/public.sql)', lineStart: 1, lineEnd: 3 })),
    readAttachment: vi.fn(async () => ({ path: 'assets/public.sql', revision: 'c'.repeat(64), fragment: null,
      content: { kind: 'text' as const, mime: 'text/plain' as const, format: 'sql', text: 'SELECT 1;' } })),
  };
  const view = render(<CanonicalReader reference={reference} backend={backend} sourceKind="local" documentActions={false} onClose={() => {}} />);
  const link = await screen.findByRole('link', { name: 'SQL' });
  const scroll = view.container.querySelector<HTMLElement>('.mdreader')!;
  scroll.scrollTop = 120;
  fireEvent.click(link);
  await screen.findByRole('region', { name: 'Linked file preview' });
  const retained = scroll.closest<HTMLElement>('.attachment-layer__origin')!;
  expect(retained.style.visibility).toBe('hidden');
  expect(retained.hasAttribute('inert')).toBe(true);
  expect(retained.getAttribute('aria-hidden')).toBe('true');
  expect(scroll.closest<HTMLElement>('.provider-reader-dialog__document')!.style.display).not.toBe('none');
  expect(backend.readAttachment).toHaveBeenCalledWith(reference, 'assets/public.sql');
  expect(view.container.querySelector('pre')!.textContent).toBe('SELECT 1;');
  fireEvent.click(screen.getByRole('button', { name: 'Return to document' }));
  expect(view.container.querySelector('.mdreader')).toBe(scroll);
  expect(scroll.scrollTop).toBe(120);
  expect(retained.style.visibility).toBe('');
  expect(retained.hasAttribute('inert')).toBe(false);
  expect(backend.readDocument).toHaveBeenCalledOnce();
});

test('missing relative files use a plain file notice; real wiki lookup misses distinguish note names', async () => {
  const reference = { sourceId: 'a'.repeat(64), path: 'index.md', revision: 'b'.repeat(64) };
  const backend: CanonicalReaderBackend = {
    readDocument: vi.fn(async () => ({ operation: 'read_document' as const, ...reference,
      markdown: '# Origin\n\n[Missing](missing.md)\n\n[[Absent|Absent note]]', lineStart: 1, lineEnd: 5 })),
    resolveLink: vi.fn(async (_reference, kind) => ({ state: 'local_matches' as const, references: [], linkKind: kind })),
  };
  render(<CanonicalReader reference={reference} backend={backend} sourceKind="local" documentActions={false} onClose={() => {}} />);
  fireEvent.click(await screen.findByRole('link', { name: 'Missing' }));
  await screen.findByText('The linked file could not be found in this folder.');
  fireEvent.click(screen.getByRole('link', { name: 'Absent note' }));
  await screen.findByText('No indexed note matches this name in this source.');
});
test('changing reading appearance restyles an already open exact cloud copy without reading it again', async () => {
  useSettingsStore.getState().reset();
  const reference = { sourceId: 'provider:' + 'a'.repeat(64), path: 'notes.md', revision: 'b'.repeat(64), title: 'Notes' };
  const backend: CanonicalReaderBackend = { readDocument: vi.fn(async () => ({
    operation: 'read_document' as const, ...reference, markdown: '# Notes\n\nOriginal paragraph.', lineStart: 1, lineEnd: 3,
  })) };
  const view = render(<CanonicalReader reference={reference} backend={backend} onClose={() => {}} documentActions={false} />);
  await screen.findByText('Original paragraph.');
  const body = view.container.querySelector<HTMLElement>('.provider-reader-dialog__document')!;
  expect(body.style.getPropertyValue('--font-reading-serif')).toBe('');
  act(() => { useSettingsStore.getState().toggleReadingFont(); useSettingsStore.getState().increaseTextSize(); });
  expect(body.style.getPropertyValue('--font-reading-serif')).toBe('var(--font-reading-sans)');
  expect(body.style.getPropertyValue('--text-0')).toBe('calc(1rem * 1.08)');
  act(() => useSettingsStore.getState().toggleReadingFont());
  expect(body.style.getPropertyValue('--font-reading-serif')).toBe('');
  expect(screen.getByText('Original paragraph.')).toBeTruthy();
  expect(backend.readDocument).toHaveBeenCalledOnce();
  view.unmount(); useSettingsStore.getState().reset();
});
test('inline local links expose exact ambiguous candidates and preserve shared previous navigation', async () => {
  const origin: ProviderReaderReference = {
    sourceId: 'a'.repeat(64), path: 'origin.md', revision: 'b'.repeat(64), title: 'Origin',
  };
  const candidates = ['one/note.md', 'two/note.md'].map((path) => ({
    ...origin, path, revision: 'c'.repeat(64), title: path, line: 3,
  }));
  const backend: CanonicalReaderBackend = {
    readDocument: vi.fn(async (reference) => ({
      operation: 'read_document' as const, ...reference,
      markdown: reference.path === origin.path ? '# Origin\n\n[Next](Shared%20title)' : '# Selected exact copy',
      lineStart: 1, lineEnd: 3,
    })),
    resolveLink: vi.fn(async () => ({ state: 'local_matches' as const, references: candidates })),
  };
  render(<CanonicalReader reference={origin} backend={backend} sourceKind="local" onClose={() => {}} />);
  fireEvent.click(await screen.findByRole('link', { name: 'Next' }));
  expect(backend.resolveLink).toHaveBeenCalledWith(origin, 'relative', 'Shared%20title');
  expect(backend.readDocument).toHaveBeenCalledOnce();
  fireEvent.click(await screen.findByRole('button', { name: 'Read exact two/note.md' }));
  await screen.findByRole('heading', { name: /Selected exact copy/ });
  expect(backend.readDocument).toHaveBeenLastCalledWith(candidates[1]);
  expect(screen.queryByRole('dialog')).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Previous document' }));
  await screen.findByRole('heading', { name: /Origin/ });
  expect(backend.readDocument).toHaveBeenLastCalledWith(origin);
});

test('restoring a saved position waits for exact receipt verification and refuses a mismatched body', async () => {
  const reference = { sourceId: 'a'.repeat(64), path: 'saved.md', revision: 'b'.repeat(64), title: 'Saved' };
  const onPosition = vi.fn();
  const backend: CanonicalReaderBackend = {
    readDocument: vi.fn(async () => ({ operation: 'read_document' as const, ...reference,
      revision: 'c'.repeat(64), markdown: '# Different revision', lineStart: 1, lineEnd: 1 })),
  };
  render(<CanonicalReader reference={reference} backend={backend} sourceKind="local" onClose={() => {}}
    position={{ revision: reference.revision, line: 1, block: 0, offset: 10 }} onPosition={onPosition} />);
  expect(await screen.findByRole('alert')).toHaveProperty('textContent', 'The selected document revision is no longer available.');
  expect(screen.queryByRole('heading', { name: /Different revision/ })).toBeNull();
  expect(onPosition).not.toHaveBeenCalled();
});

test('an exact capture read disables local editing and archive comparison even after history restore', async () => {
  const reference = { sourceId: 'a'.repeat(64), path: 'outputs/plan.md', revision: 'b'.repeat(64), title: 'Captured plan' };
  const backend: CanonicalReaderBackend = { readDocument: vi.fn(async () => ({ operation: 'read_document' as const,
    ...reference, sourceKind: 'capture' as const, markdown: '# Visible plan', lineStart: 1, lineEnd: 1 })) };
  const edit = vi.fn(); const verified = vi.fn();
  render(<CanonicalReader reference={reference} backend={backend} sourceKind="local" onClose={() => {}} onEdit={edit} onVerifiedRead={verified} />);
  await screen.findByRole('heading', { name: /Visible plan/ });
  expect(screen.queryByRole('button', { name: 'Edit local file' })).toBeNull();
  expect(screen.queryByRole('button', { name: 'Archived reviews…' })).toBeNull();
  expect(screen.getByText(/exact captured visible-output revision/)).toBeTruthy();
  expect(edit).not.toHaveBeenCalled(); expect(verified).toHaveBeenLastCalledWith(reference, 'capture');
});

test('three visible local images load without clicks while external images remain blocked and close releases every URL', async () => {
  const NativeURL = URL;
  const revoke = vi.fn();
  let sequence = 1;
  vi.stubGlobal('URL', class extends NativeURL {
    static override createObjectURL = vi.fn(() => `blob:guarded-local-image-${sequence++}`);
    static override revokeObjectURL = revoke;
  });
  vi.stubGlobal('IntersectionObserver', class {
    private disconnected = false;
    constructor(private callback: IntersectionObserverCallback) {}
    observe(target: Element) {
      queueMicrotask(() => {
        const rect = target.getBoundingClientRect();
        const entry: IntersectionObserverEntry = {
          target, isIntersecting: true, intersectionRatio: 1,
          boundingClientRect: rect, intersectionRect: rect,
          rootBounds: null, time: performance.now(),
        };
        if (!this.disconnected) this.callback([entry], this as unknown as IntersectionObserver);
      });
    }
    disconnect() { this.disconnected = true; }
    unobserve() {}
  });
  const fetch = vi.fn(); vi.stubGlobal('fetch', fetch);
  const reference = { sourceId: 'a'.repeat(64), path: 'welcome.md', revision: 'b'.repeat(64), title: 'Welcome' };
  const backend: CanonicalReaderBackend = {
    readDocument: vi.fn(async () => ({ operation: 'read_document' as const, ...reference,
      sourceKind: 'local' as const, markdown: '# Welcome\n\n![First](images/one.png)\n\n![Second](images/two.png)\n\n![Third](images/three.png)\n\n![Remote](https://example.invalid/pixel.png)', lineStart: 1, lineEnd: 9 })),
    readImage: vi.fn(async () => ({ mime: 'image/png' as const, width: 1, height: 1, base64: 'iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+ip1sAAAAASUVORK5CYII=' })),
  };
  const view = render(<CanonicalReader reference={reference} backend={backend} sourceKind="local" onClose={() => {}} />);
  await screen.findByRole('img', { name: 'Third' });
  for (const name of ['First', 'Second', 'Third']) expect(screen.getByRole('img', { name }).getAttribute('src')).toMatch(/^blob:guarded-local-image-/);
  expect(backend.readImage).toHaveBeenCalledTimes(3);
  for (const name of ['one', 'two', 'three']) expect(backend.readImage).toHaveBeenCalledWith(reference, `images/${name}.png`);
  expect(revoke).not.toHaveBeenCalled();
  expect(screen.queryByRole('img', { name: 'Remote' })).toBeNull();
  expect(fetch).not.toHaveBeenCalled();
  view.unmount();
  expect(revoke).toHaveBeenCalledTimes(3);
});

test('a real local edit action replaces the contradictory read-only toolbar label', async () => {
  const reference = { sourceId: 'a'.repeat(64), path: 'note.md', revision: 'b'.repeat(64), title: 'Note' };
  const backend: CanonicalReaderBackend = { readDocument: vi.fn(async () => ({ operation: 'read_document' as const,
    ...reference, sourceKind: 'local' as const, markdown: '# Local note', lineStart: 1, lineEnd: 1 })) };
  const edit = vi.fn();
  render(<CanonicalReader reference={reference} backend={backend} sourceKind="local" onClose={() => {}} onEdit={edit} />);
  await screen.findByRole('heading', { name: /Local note/ });
  fireEvent.click(screen.getByRole('button', { name: 'Edit local file' }));
  expect(edit).toHaveBeenCalledWith(reference);
  expect(screen.queryByText('Read only')).toBeNull();
});

test('a provider resolved heading survives navigation and beats a prior saved position in the exact new document', async () => {
  const origin = { sourceId: 'provider:' + 'a'.repeat(64), path: 'origin.md', revision: 'b'.repeat(64), title: 'Origin' };
  const next = { ...origin, path: 'next.md', revision: 'c'.repeat(64), title: 'Next' };
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    const scroll = this.closest<HTMLElement>('.mdreader');
    return { top: this.id === 'user-content-repeated-1' ? 700 - (scroll?.scrollTop ?? 0) : 0 } as DOMRect;
  });
  const backend: CanonicalReaderBackend = {
    readDocument: vi.fn(async (reference) => ({ operation: 'read_document' as const, ...reference,
      markdown: reference.path === origin.path ? '[Next](next.md#repeated-1)' : '## Repeated\n\nFirst.\n\n## Repeated\n\nSecond.', lineStart: 1, lineEnd: 7 })),
    resolveLink: vi.fn(async () => ({ state: 'resolved' as const,
      projection: { jobId: 'public-fixture', readerSourceId: next.sourceId, path: next.path, revision: next.revision },
      candidate: { sourceId: 'public-source', documentId: 'public-document', title: 'Next' }, line: 1, heading: 'repeated-1' })),
  };
  const view = render(<CanonicalReader reference={origin} backend={backend} onClose={() => {}}
    documentActions={false} position={{ revision: next.revision, line: 1, block: 0, offset: 90 }} />);
  fireEvent.click(await screen.findByRole('link', { name: 'Next' }));
  await screen.findByText('Second.');
  expect(backend.readDocument).toHaveBeenLastCalledWith(expect.objectContaining({ ...next, heading: 'repeated-1' }));
  expect(view.container.querySelector<HTMLElement>('.mdreader')!.scrollTop).toBe(700);
  expect(screen.queryByText(/requested location is unavailable/)).toBeNull();
});

test('canonical footnote and return links use actual fragment targets without trying to resolve another document', async () => {
  const reference = { sourceId: 'a'.repeat(64), path: 'notes.md', revision: 'b'.repeat(64), title: 'Note' };
  const backend: CanonicalReaderBackend = {
    readDocument: vi.fn(async () => ({ operation: 'read_document' as const, ...reference,
      markdown: '# Note\n\nFact[^one].\n\n[^one]: Definition.', lineStart: 1, lineEnd: 5 })), resolveLink: vi.fn(),
  };
  const view = render(<CanonicalReader reference={reference} backend={backend} sourceKind="local" onClose={() => {}} documentActions={false} />);
  await screen.findByText('Definition.');
  fireEvent.click(view.container.querySelector('a[href="#user-content-fn-one"]')!);
  expect(screen.queryByText(/requested location is unavailable/)).toBeNull();
  fireEvent.click(view.container.querySelector('a[href="#user-content-fnref-one"]')!);
  expect(backend.resolveLink).not.toHaveBeenCalled(); expect(backend.readDocument).toHaveBeenCalledOnce();
});

test('a missing cross-document fragment still reads the exact linked document and reports the location separately', async () => {
  const reference = { sourceId: 'a'.repeat(64), path: 'next.md', revision: 'b'.repeat(64), title: 'Next', heading: 'missing' };
  const backend: CanonicalReaderBackend = { readDocument: vi.fn(async () => ({ operation: 'read_document' as const,
    ...reference, markdown: '# Available body\n\nThe document is readable.', lineStart: 1, lineEnd: 3 })) };
  render(<CanonicalReader reference={reference} backend={backend} sourceKind="local" onClose={() => {}} documentActions={false}
    position={{ revision: reference.revision, line: 1, block: 0, offset: 300 }} />);
  await screen.findByText('The document is readable.');
  expect(screen.getByRole('status').textContent).toContain('requested location is unavailable');
});
