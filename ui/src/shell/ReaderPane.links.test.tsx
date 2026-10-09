import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { ReaderPane } from './ReaderPane';
import type { ReaderPaneProps } from './ReaderPane';
import { readDoc, resolveWikilink } from '../platform/fs';
import { useEditorStore } from '../stores/editor';
vi.mock('../platform/fs', async (original) => ({ ...await original<typeof import('../platform/fs')>(), readDoc: vi.fn(), resolveWikilink: vi.fn() }));
vi.mock('./SourcePane', () => ({ SourcePane: () => <div>Editor boundary</div> }));
afterEach(() => { cleanup(); useEditorStore.getState().reset(); vi.restoreAllMocks(); vi.unstubAllGlobals(); vi.mocked(readDoc).mockReset(); vi.mocked(resolveWikilink).mockReset(); });
function setup(markdown: string, split = false) {
  vi.stubGlobal('requestAnimationFrame', (callback: FrameRequestCallback) => window.setTimeout(() => callback(0), 0));
  vi.stubGlobal('cancelAnimationFrame', (id: number) => window.clearTimeout(id));
  vi.mocked(readDoc).mockImplementation(async (path) => ({ path, content: markdown, hash: 'public', mtime: 0 }));
  vi.mocked(resolveWikilink).mockResolvedValue({ status: 'unresolved' });
  const props: ReaderPaneProps = {
    tab: { id: 'public-tab', docRef: { path: '/public/index.md', name: 'index.md' }, scroll: { block: 0, offset: 0 }, history: [], historyIndex: 0, pinned: false, originRoot: '/public' },
    root: '/public', files: [], outlineVisible: false, readingFont: 'serif', textScale: 1, reloadNonce: 0,
    onToggleOutline: vi.fn(), onNavigate: vi.fn(), onPersistScroll: vi.fn(), onDocMeta: vi.fn(), onPickFolder: vi.fn(),
    onMissingWikiLink: vi.fn(), onAmbiguousWikiLink: vi.fn(), onSetMode: vi.fn(), onSaveActive: vi.fn(), onReloadActive: vi.fn(), onOverwriteActive: vi.fn(),
  };
  if (split) useEditorStore.getState().seed('public-tab', '/public/index.md', markdown, 'public', 'split');
  return props;
}
test('split preview fragment clicks scroll the mounted preview, including footnotes', async () => {
  const props = setup('# Note\n\nFact[^one].\n\n[^one]: Definition.', true);
  const view = render(<ReaderPane {...props} />);
  await screen.findByText('Definition.');
  const scroll = view.container.querySelector<HTMLElement>('.mdreader')!;
  scroll.getBoundingClientRect = () => ({ top: 100 }) as DOMRect;
  const target = view.container.querySelector<HTMLElement>('[id="user-content-fn-one"]')!;
  target.getBoundingClientRect = () => ({ top: 800 - scroll.scrollTop }) as DOMRect;
  fireEvent.click(view.container.querySelector('a[href="#user-content-fn-one"]')!);
  expect(scroll.scrollTop).toBe(700);
  expect(screen.queryByText(/requested location is unavailable/)).toBeNull();
});
test('a title-resolved wiki fragment bypasses approximate heading lookup and jumps after exact target rendering', async () => {
  const props = setup('[[Notebook#fn-one|Notebook definition]]');
  const view = render(<ReaderPane {...props} />);
  await screen.findByRole('link', { name: 'Notebook definition' });
  vi.mocked(resolveWikilink).mockResolvedValue({ status: 'resolved', path: '/public/physical.md', name: 'physical.md', line: 1 });
  fireEvent.click(screen.getByRole('link', { name: 'Notebook definition' }));
  await waitFor(() => expect(props.onNavigate).toHaveBeenCalledWith({ path: '/public/physical.md', name: 'physical.md' }));
  expect(resolveWikilink).toHaveBeenLastCalledWith('/public/index.md', 'Notebook', null);
  vi.mocked(readDoc).mockResolvedValue({ content: 'A fact[^one].\n\n[^one]: Definition.', hash: 'public', mtime: 0 });
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function (this: HTMLElement) {
    return { top: this.id === 'user-content-fn-one' ? 650 - (this.closest<HTMLElement>('.mdreader')?.scrollTop ?? 0) : 0 } as DOMRect;
  });
  view.rerender(<ReaderPane {...props} tab={{ ...props.tab!, docRef: { path: '/public/physical.md', name: 'physical.md' } }} />);
  await screen.findByText('Definition.');
  await waitFor(() => expect(view.container.querySelector<HTMLElement>('.mdreader')!.scrollTop).toBe(650));
});
test('a newer anchor cancels an earlier same-document async wiki resolution', async () => {
  const props = setup('# Here\n\n[[Later|Later note]]\n\n[Stay](#here)');
  const view = render(<ReaderPane {...props} />);
  await screen.findByRole('link', { name: 'Later note' });
  let finish!: (value: Awaited<ReturnType<typeof resolveWikilink>>) => void;
  vi.mocked(resolveWikilink).mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  fireEvent.click(screen.getByRole('link', { name: 'Later note' }));
  fireEvent.click(screen.getByRole('link', { name: 'Stay' }));
  await act(async () => finish({ status: 'resolved', path: '/public/later.md', name: 'later.md', line: 1 }));
  expect(props.onNavigate).not.toHaveBeenCalled();
  expect(view.container.querySelector('h1')!.textContent).toContain('Here');
});
