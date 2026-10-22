import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { AgentSessionsPane } from './AgentSessionsPane';
import type { AgentActivityBackend, AgentActivityFile, AgentActivitySession } from './contracts';

const NOW = Date.UTC(2026, 9, 21, 12);
const source = 'a'.repeat(64), revision = 'b'.repeat(64);
const session = (id: string, patch: Partial<AgentActivitySession> = {}): AgentActivitySession => ({
  id, agent: 'claude', title: `Session ${id}`, titleSource: 'ai', cwd: '/public/repo', gitBranch: 'main',
  startedAtMs: NOW - 3_600_000, lastActivityMs: NOW - 3_600_000, endedAtMs: NOW - 3_000_000, fileCount: 1, ...patch,
});
const file = (displayPath: string, patch: Partial<AgentActivityFile> = {}): AgentActivityFile => ({
  absPath: `/public/repo/${displayPath}`, displayPath, op: 'edit', firstAtMs: NOW, lastAtMs: NOW, exists: true, isPlan: false,
  reference: { sourceId: source, path: displayPath, revision }, ...patch,
});

function fake(sessions: AgentActivitySession[], files: Record<string, AgentActivityFile[]> = {}) {
  let emit: (id: string | null) => void = () => {};
  const backend = {
    list: vi.fn<AgentActivityBackend['list']>(async ({ agent, offset, limit, filter }) => {
      const rows = sessions.filter(row => row.agent === agent && (!filter || row.title.includes(filter)));
      const page = rows.slice(offset, offset + limit);
      return { sessions: page, nextOffset: offset + limit < rows.length ? offset + limit : null, total: rows.filter(row => row.fileCount > 0).length };
    }),
    files: vi.fn<AgentActivityBackend['files']>(async id => files[id] ?? []),
    rename: vi.fn(async () => {}), hide: vi.fn(async () => {}), revealCwd: vi.fn(async () => {}),
    trackingState: vi.fn(async () => ({ claude: { enabled: true, available: true }, codex: { enabled: true, available: true } })),
    setTracking: vi.fn(), loadOlder: vi.fn(async () => {}), forget: vi.fn(async () => {}),
    onChanged: vi.fn(async (handler: (id: string | null) => void) => { emit = handler; return () => { emit = () => {}; }; }),
  } satisfies AgentActivityBackend;
  return { backend, emit: (id: string | null) => emit(id), sessions, files };
}
const tree = () => within(screen.getByRole('tree', { name: 'Agent sessions' }));
afterEach(() => { cleanup(); vi.useRealTimers(); });

test('groups by agent, hides sessions without Markdown and opens exact files from an expanded live session', async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true, now: NOW });
  const api = fake([
    session('live', { title: 'Fix CI desktop e2e', lastActivityMs: NOW - 120_000, endedAtMs: null }),
    session('empty', { title: 'Nothing written', fileCount: 0 }),
    session('codex-one', { agent: 'codex', title: 'Reader web sharing', lastActivityMs: NOW - 86_400_000 }),
  ], { live: [file('twinkling-mixing-wren.md', { isPlan: true }), file('docs/development/ci.md'), file('gone.md', { exists: false, reference: null })] });
  const onOpen = vi.fn();
  render(<AgentSessionsPane backend={api.backend} onOpen={onOpen} onManage={vi.fn()} />);
  const live = await screen.findByRole('treeitem', { name: 'Fix CI desktop e2e, live' });
  expect(tree().getByRole('treeitem', { name: 'Claude Code, 1 sessions' })).toBeTruthy();
  expect(tree().getByRole('treeitem', { name: 'Codex, 1 sessions' })).toBeTruthy();
  expect(tree().queryByText('Nothing written')).toBeNull();
  expect(within(live).getByText('2m')).toBeTruthy();
  expect(tree().getByText('1d')).toBeTruthy();
  fireEvent.click(live.querySelector('[data-tree-row]')!);
  const plan = await screen.findByRole('treeitem', { name: 'twinkling-mixing-wren.md, plan' });
  expect(live.getAttribute('aria-expanded')).toBe('true');
  expect(api.backend.files).toHaveBeenCalledWith('live');
  const deleted = tree().getByRole('treeitem', { name: 'gone.md, deleted' });
  expect(deleted.getAttribute('aria-disabled')).toBe('true');
  fireEvent.click(deleted.querySelector('[data-tree-row]')!);
  expect(onOpen).not.toHaveBeenCalled();
  fireEvent.click(tree().getByRole('treeitem', { name: 'ci.md' }).querySelector('[data-tree-row]')!);
  expect(onOpen).toHaveBeenCalledWith(expect.objectContaining({ sourceId: source, path: 'docs/development/ci.md', revision, title: 'ci.md' }));
  expect(plan).toBeTruthy();
});

test('tracking off with no sessions explains how to start and opens Agents settings', async () => {
  const api = fake([]);
  api.backend.trackingState.mockResolvedValue({ claude: { enabled: false, available: true }, codex: { enabled: false, available: false } });
  const onManage = vi.fn();
  render(<AgentSessionsPane backend={api.backend} onOpen={vi.fn()} onManage={onManage} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Turn on session tracking' }));
  expect(onManage).toHaveBeenCalledOnce();
  expect(screen.queryByRole('tree')).toBeNull();
  expect(screen.queryByRole('searchbox')).toBeNull();
});

test('native change events refresh rows and expanded files without polling', async () => {
  const api = fake([session('one')], { one: [file('a.md')] });
  render(<AgentSessionsPane backend={api.backend} onOpen={vi.fn()} onManage={vi.fn()} />);
  fireEvent.click((await screen.findByRole('treeitem', { name: 'Session one' })).querySelector('[data-tree-row]')!);
  await screen.findByRole('treeitem', { name: 'a.md' });
  await waitFor(() => expect(api.backend.onChanged).toHaveBeenCalled());
  const listCalls = api.backend.list.mock.calls.length;
  api.sessions.push(session('two', { title: 'Newer work', lastActivityMs: NOW }));
  api.files.one = [file('a.md'), file('b.md', { op: 'create' })];
  act(() => { api.emit('one'); api.emit('two'); });
  await screen.findByRole('treeitem', { name: 'b.md' });
  await screen.findByRole('treeitem', { name: 'Newer work' });
  // Two events coalesce into one refresh per agent.
  expect(api.backend.list.mock.calls.length).toBe(listCalls + 2);
  // Without another event nothing re-reads the backend.
  const settled = [api.backend.list.mock.calls.length, api.backend.files.mock.calls.length];
  await act(() => new Promise(done => setTimeout(done, 400)));
  expect([api.backend.list.mock.calls.length, api.backend.files.mock.calls.length]).toEqual(settled);
});

test('shows 5 sessions per agent, live ones always, then reveals 20 more and sends filters to native', async () => {
  vi.useFakeTimers({ shouldAdvanceTime: true, now: NOW });
  const api = fake(Array.from({ length: 25 }, (_, index) => session(`s${index}`, {
    title: index === 3 ? 'Special plan' : `Work ${index}`, ...(index === 9 ? { endedAtMs: null, lastActivityMs: NOW - 60_000 } : {}) })));
  render(<AgentSessionsPane backend={api.backend} onOpen={vi.fn()} onManage={vi.fn()} />);
  await screen.findByRole('treeitem', { name: 'Claude Code, 25 sessions' });
  expect(tree().getByText('Work 4')).toBeTruthy(); expect(tree().queryByText('Work 5')).toBeNull();
  expect(tree().getByRole('treeitem', { name: 'Work 9, live' })).toBeTruthy();
  fireEvent.click(tree().getByText('Show 19 more'));
  await screen.findByText('Work 24');
  expect(api.backend.list).toHaveBeenCalledWith({ agent: 'claude', offset: 20, limit: 20 });
  expect(tree().queryByText(/^Show/)).toBeNull();
  fireEvent.change(screen.getByRole('searchbox', { name: 'Filter sessions and files' }), { target: { value: 'Special' } });
  await waitFor(() => expect(api.backend.list).toHaveBeenCalledWith({ agent: 'claude', offset: 0, limit: 20, filter: 'Special' }));
  await waitFor(() => expect(tree().queryByText('Work 1')).toBeNull());
  expect(tree().getByText('Special plan')).toBeTruthy();
  expect(tree().queryByRole('treeitem', { name: /^Codex/ })).toBeNull();
  fireEvent.change(screen.getByRole('searchbox'), { target: { value: 'zzz' } });
  expect(await screen.findByText('No sessions or files match “zzz”.')).toBeTruthy();
});

test('file rows show names only, with the parent folder when two files share a name', async () => {
  const api = fake([session('one')], { one: [file('docs/README.md'), file('ui/README.md'), file('notes/plan.md', { op: 'create' })] });
  render(<AgentSessionsPane backend={api.backend} onOpen={vi.fn()} onManage={vi.fn()} />);
  fireEvent.click((await screen.findByRole('treeitem', { name: 'Session one' })).querySelector('[data-tree-row]')!);
  const docs = await screen.findByRole('treeitem', { name: 'README.md, in docs' });
  expect(docs.querySelector('[data-tree-row]')!.getAttribute('title')).toBe('/public/repo/docs/README.md');
  expect(tree().getByRole('treeitem', { name: 'README.md, in ui' })).toBeTruthy();
  const plan = tree().getByRole('treeitem', { name: 'plan.md' });
  expect(plan.textContent).toBe('plan.md');
});

test('keyboard moves through the tree, expands sessions and drives the context menu', async () => {
  const api = fake([session('one', { title: 'First' }), session('two', { title: 'Second' })], { one: [file('a.md')] });
  render(<AgentSessionsPane backend={api.backend} onOpen={vi.fn()} onManage={vi.fn()} />);
  const first = await screen.findByRole('treeitem', { name: 'First' });
  const group = tree().getByRole('treeitem', { name: 'Claude Code, 2 sessions' });
  expect(group.tabIndex).toBe(0); expect(first.tabIndex).toBe(-1);
  group.focus();
  fireEvent.keyDown(group, { key: 'ArrowDown' });
  expect(document.activeElement).toBe(first);
  expect(first.tabIndex).toBe(0); expect(group.tabIndex).toBe(-1);
  fireEvent.keyDown(first, { key: 'ArrowRight' });
  await screen.findByRole('treeitem', { name: 'a.md' });
  fireEvent.keyDown(first, { key: 'ArrowRight' });
  expect(document.activeElement).toBe(tree().getByRole('treeitem', { name: 'a.md' }));
  fireEvent.keyDown(document.activeElement!, { key: 'ArrowLeft' });
  expect(document.activeElement).toBe(first);
  fireEvent.keyDown(first, { key: 'ArrowLeft' });
  expect(first.getAttribute('aria-expanded')).toBe('false');
  fireEvent.keyDown(first, { key: 'F10', shiftKey: true });
  const menu = screen.getByRole('menu', { name: 'Actions for First' });
  expect(document.activeElement).toBe(within(menu).getByRole('menuitem', { name: 'Rename…' }));
  expect(within(menu).getByRole('menuitem', { name: 'Reveal folder' })).toBeTruthy();
  fireEvent.click(within(menu).getByRole('menuitem', { name: 'Rename…' }));
  const input = screen.getByRole('textbox', { name: 'Session name' });
  fireEvent.change(input, { target: { value: '  Renamed  ' } });
  fireEvent.keyDown(input, { key: 'Enter' });
  await screen.findByRole('treeitem', { name: 'Renamed' });
  expect(api.backend.rename).toHaveBeenCalledWith('one', 'Renamed');
  const second = tree().getByRole('treeitem', { name: 'Second' });
  fireEvent.contextMenu(second.querySelector('[data-tree-row]')!);
  fireEvent.click(within(screen.getByRole('menu')).getByRole('menuitem', { name: 'Hide session' }));
  await waitFor(() => expect(tree().queryByRole('treeitem', { name: 'Second' })).toBeNull());
  expect(api.backend.hide).toHaveBeenCalledWith('two');
});

test('a failed rename keeps the title and reports the failure', async () => {
  const api = fake([session('one', { title: 'First' })]);
  api.backend.rename.mockRejectedValueOnce(new Error('/private/path'));
  render(<AgentSessionsPane backend={api.backend} onOpen={vi.fn()} onManage={vi.fn()} />);
  const first = await screen.findByRole('treeitem', { name: 'First' });
  fireEvent.keyDown(first, { key: 'F2' });
  const input = screen.getByRole('textbox', { name: 'Session name' });
  fireEvent.change(input, { target: { value: 'Other' } }); fireEvent.blur(input);
  expect((await screen.findByRole('alert')).textContent).toBe('The session could not be renamed.');
  expect(tree().getByRole('treeitem', { name: 'First' })).toBeTruthy();
});
