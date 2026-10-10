import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { ReaderPane } from './ReaderPane';
import type { ReaderPaneProps } from './ReaderPane';
import { readDoc, resolveWikilink } from '../platform/fs';
import { useEditorStore } from '../stores/editor';
import { commands, type WorkspaceOriginReceipt, type AssetPreview } from '../bindings';
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
const origin: WorkspaceOriginReceipt = { epoch: '11111111-1111-4111-8111-111111111111', path: 'index.md', hash: 'a'.repeat(64) };
const raster: AssetPreview = { path: 'assets/diagram.png', revision: 'b'.repeat(64), fragment: null,
  content: { kind: 'raster', mime: 'image/png', width: 1, height: 1, base64: 'AA==' } };

test.each([false, true])('workspace read/split uses the native saved-origin image adapter without file URLs (split=%s)', async (split) => {
  const props = setup('# Note\n\n![Public](assets/diagram.png)', split);
  vi.mocked(readDoc).mockResolvedValue({ content: '# Note\n\n![Public](assets/diagram.png)', mtime: 0, hash: origin.hash, originReceipt: origin });
  if (split) useEditorStore.getState().edit('public-tab', '/public/index.md', '# Note\n\n![Public](assets/diagram.png)\n\nPublic live preview is ready.');
  const native = vi.spyOn(commands, 'readerWorkspaceAsset').mockResolvedValue({ status: 'ok', data: raster });
  const NativeURL = URL;
  const revoke = vi.fn();
  vi.stubGlobal('URL', class extends NativeURL {
    static override createObjectURL = vi.fn(() => 'blob:public-workspace-image');
    static override revokeObjectURL = revoke;
  });
  const view = render(<ReaderPane {...props} />);
  // The saved AST can render before the existing debounced draft parse. Wait
  // for that live preview so loading isn't discarded by an unrelated remount.
  if (split) await screen.findByText('Public live preview is ready.');
  fireEvent.click(await screen.findByRole('button', { name: 'Load local image' }));
  await waitFor(() => expect(view.container.querySelector('img')?.getAttribute('src')).toBe('blob:public-workspace-image'));
  expect(native).toHaveBeenCalledWith({ origin, target: 'assets/diagram.png', kind: 'image' });
  expect(view.container.querySelector('[src^="asset:"], [src^="file:"], [src^="http:"]')).toBeNull();
  view.unmount();
  expect(revoke).toHaveBeenCalledWith('blob:public-workspace-image');
});

test('split attachment returns to the exact mounted preview and leaves an unsaved draft untouched', async () => {
  const saved = '# Note\n\n[SQL](assets/public.sql)';
  const props = setup(saved, true);
  vi.mocked(readDoc).mockResolvedValue({ content: saved, mtime: 0, hash: origin.hash, originReceipt: origin });
  useEditorStore.getState().edit('public-tab', '/public/index.md', saved + '\n\nUnsaved words');
  const native = vi.spyOn(commands, 'readerWorkspaceAsset').mockResolvedValue({ status: 'ok', data: {
    path: 'assets/public.sql', revision: 'c'.repeat(64), fragment: null,
    content: { kind: 'text', mime: 'text/plain', format: 'sql', text: 'SELECT 1;' },
  } });
  const view = render(<ReaderPane {...props} />);
  await screen.findByText('Unsaved words');
  const reader = view.container.querySelector<HTMLElement>('.mdreader')!;
  reader.scrollTop = 97;
  const link = screen.getByRole('link', { name: 'SQL' });
  link.focus();
  const focus = vi.spyOn(link, 'focus');
  fireEvent.click(link);
  await screen.findByRole('region', { name: 'Linked file preview' });
  const retained = reader.closest<HTMLElement>('.attachment-layer__origin')!;
  expect(retained.style.visibility).toBe('hidden');
  expect(retained.hasAttribute('inert')).toBe(true);
  expect(retained.getAttribute('aria-hidden')).toBe('true');
  expect(reader.closest<HTMLElement>('.reader-body')!.style.display).not.toBe('none');
  expect(native).toHaveBeenCalledWith({ origin, target: 'assets/public.sql', kind: 'attachment' });
  fireEvent.click(screen.getByRole('button', { name: 'Return to document' }));
  expect(view.container.querySelector('.mdreader')).toBe(reader);
  expect(reader.scrollTop).toBe(97);
  expect(retained.style.visibility).toBe('');
  expect(retained.hasAttribute('inert')).toBe(false);
  expect(document.activeElement).toBe(link);
  expect(focus).toHaveBeenCalledWith({ preventScroll: true });
  expect(useEditorStore.getState().entry('public-tab', '/public/index.md')?.buffer).toContain('Unsaved words');
});

test('a late attachment read cannot appear after changing the selected document', async () => {
  const saved = '# Origin\n\n[SQL](assets/public.sql)';
  const props = setup(saved);
  vi.mocked(readDoc).mockResolvedValue({ content: saved, mtime: 0, hash: origin.hash, originReceipt: origin });
  let finish!: (value: { status: 'ok'; data: AssetPreview }) => void;
  vi.spyOn(commands, 'readerWorkspaceAsset').mockImplementation(() => new Promise((resolve) => { finish = resolve; }));
  const view = render(<ReaderPane {...props} />);
  fireEvent.click(await screen.findByRole('link', { name: 'SQL' }));
  vi.mocked(readDoc).mockResolvedValue({ content: '# New selection', mtime: 0, hash: 'd'.repeat(64), originReceipt: { ...origin, path: 'next.md', hash: 'd'.repeat(64) } });
  view.rerender(<ReaderPane {...props} tab={{ ...props.tab!, docRef: { path: '/public/next.md', name: 'next.md' } }} />);
  await screen.findByRole('heading', { name: /New selection/ });
  await act(async () => finish({ status: 'ok', data: { path: 'assets/public.sql', revision: 'c'.repeat(64), fragment: null, content: { kind: 'text', mime: 'text/plain', format: 'sql', text: 'Old selection bytes' } } }));
  expect(screen.queryByRole('region', { name: 'Linked file preview' })).toBeNull();
  expect(screen.queryByText('Old selection bytes')).toBeNull();
});

test('an unsaved newly authored image target refusal preserves the split draft', async () => {
  const props = setup('# Saved', true);
  vi.mocked(readDoc).mockResolvedValue({ content: '# Saved', mtime: 0, hash: origin.hash, originReceipt: origin });
  useEditorStore.getState().edit('public-tab', '/public/index.md', '# Saved\n\n![New draft image](assets/new.png)');
  vi.spyOn(commands, 'readerWorkspaceAsset').mockResolvedValue({ status: 'error', error: { kind: 'unauthorized' } });
  render(<ReaderPane {...props} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Load local image' }));
  await screen.findByRole('alert');
  expect(useEditorStore.getState().entry('public-tab', '/public/index.md')?.buffer).toContain('assets/new.png');
  expect(screen.getByRole('alert').textContent).toContain('Save new links before opening them');
});
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

test('unindexed duplicate wikilink filenames require choosing the exact admitted copy', async () => {
  const props = setup('[[Guide#section|Guide section]]');
  props.files = ['/public/a/Guide.md', '/public/b/Guide.markdown'].map(path => ({
    path, name: path.split('/').at(-1)!, dir: path.slice(0, path.lastIndexOf('/')), mtime: 0, size: 1, ignored: false,
  }));
  render(<ReaderPane {...props} />);
  fireEvent.click(await screen.findByRole('link', { name: 'Guide section' }));
  const chosen = await screen.findByRole('button', { name: 'Guide.markdown — b' });
  expect(screen.getByRole('button', { name: 'Guide.md — a' })).toBeTruthy();
  expect(props.onNavigate).not.toHaveBeenCalled();
  fireEvent.click(chosen);
  expect(props.onNavigate).toHaveBeenCalledTimes(1);
  expect(props.onNavigate).toHaveBeenCalledWith({ path: '/public/b/Guide.markdown', name: 'Guide.markdown' });
});

test('the previous rendered document cannot navigate while the new document loads', async () => {
  const props = setup('# Origin\n\n[Old target](next.md)');
  props.files = [{ path: '/public/next.md', name: 'next.md', dir: '/public', mtime: 0, size: 1, ignored: false }];
  const view = render(<ReaderPane {...props} />);
  await screen.findByRole('link', { name: 'Old target' });
  vi.mocked(readDoc).mockImplementation(() => new Promise(() => {}));
  view.rerender(<ReaderPane {...props} tab={{ ...props.tab!, docRef: { path: '/public/loading.md', name: 'loading.md' } }} />);
  // The loader intentionally retains the previous rendered body while waiting.
  fireEvent.click(screen.getByRole('link', { name: 'Old target' }));
  expect(props.onNavigate).not.toHaveBeenCalled();
  expect(screen.getByRole('status').textContent).toContain('selected document is still loading');
});

test('a same-filename workspace switch discards the old async wikilink and drops its rendered body', async () => {
  const props = setup('# Origin\n\n[[Next|Old target]]');
  const view = render(<ReaderPane {...props} />);
  await screen.findByRole('link', { name: 'Old target' });
  let finish!: (value: Awaited<ReturnType<typeof resolveWikilink>>) => void;
  vi.mocked(resolveWikilink).mockImplementation(() => new Promise(resolve => { finish = resolve; }));
  fireEvent.click(screen.getByRole('link', { name: 'Old target' }));
  vi.mocked(readDoc).mockImplementation(() => new Promise(() => {}));
  view.rerender(<ReaderPane {...props} root="/different-public-root" tab={{ ...props.tab!,
    docRef: { path: '/different-public-root/index.md', name: 'index.md' }, originRoot: '/different-public-root',
  }} />);
  expect(screen.queryByRole('link', { name: 'Old target' })).toBeNull();
  await act(async () => finish({ status: 'resolved', path: '/public/next.md', name: 'next.md', line: 1 }));
  expect(props.onNavigate).not.toHaveBeenCalled();
});
