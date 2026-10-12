import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { AgentProjects } from './AgentProjects';
import type { AgentProjectsBackend, AgentProjectDocument } from './backend';
const source = 'a'.repeat(64), revision = 'b'.repeat(64);
const all: AgentProjectDocument[] = [{ source_id: source, path: 'reported.md', title: 'Reported note', revision, session_ids: ['session-one', 'session-two'] }, { source_id: source, path: 'unreported.md', title: 'Unreported note', revision, session_ids: [] }];
function backend(): AgentProjectsBackend { return { projects: vi.fn(async () => [{ id: source, name: 'Public project', enabled: true, monitoring: true }]), sessions: vi.fn<AgentProjectsBackend['sessions']>(async () => [{ id: 'session-one', project_id: source, agent: 'codex', title: 'Write notes', started_at_ms: 1, last_seen_ms: 2, ended_at_ms: null }, { id: 'session-two', project_id: source, agent: 'claude', title: 'Refine notes', started_at_ms: 3, last_seen_ms: 4, ended_at_ms: null }]), documents: vi.fn(async (_project, session) => session ? [all[0]!] : all) }; }
afterEach(() => { cleanup(); vi.useRealTimers(); });
test('Projects → sessions → documents retains unreported files in All documents and opens exact receipts', async () => {
  const api = backend(), onOpen = vi.fn(); render(<AgentProjects backend={api} onOpen={onOpen} onManage={vi.fn()} />);
  expect(api.projects).not.toHaveBeenCalled(); fireEvent.click(screen.getByRole('button', { name: 'Projects' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Public project' }));
  await screen.findByRole('button', { name: 'Unreported note' });
  expect(screen.getAllByRole('button', { name: /Reported note/ })).toHaveLength(1);
  fireEvent.click(screen.getByRole('button', { name: /Write notes/ }));
  await waitFor(() => expect(api.documents).toHaveBeenLastCalledWith(source, 'session-one'));
  await screen.findByRole('button', { name: 'Reported note' }); expect(screen.queryByRole('button', { name: 'Unreported note' })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Reported note' }));
  expect(onOpen).toHaveBeenCalledWith({ sourceId: source, path: 'reported.md', revision, title: 'Reported note', line: 1 });
  fireEvent.click(screen.getByRole('button', { name: 'All documents' })); await screen.findByRole('button', { name: 'Unreported note' });
});
test('late project response cannot replace a newer project selection', async () => {
  const api = backend(); api.projects = vi.fn(async () => [{ id: source, name: 'One', enabled: true, monitoring: true }, { id: 'c'.repeat(64), name: 'Two', enabled: true, monitoring: true }]);
  let settle!: (value: AgentProjectDocument[]) => void; vi.mocked(api.documents).mockImplementationOnce(() => new Promise(done => { settle = done; })).mockResolvedValue([]);
  render(<AgentProjects backend={api} onOpen={vi.fn()} onManage={vi.fn()} />); fireEvent.click(screen.getByRole('button', { name: 'Projects' }));
  fireEvent.click(await screen.findByRole('button', { name: 'One' })); await waitFor(() => expect(api.documents).toHaveBeenCalledOnce());
  fireEvent.click(screen.getByRole('button', { name: 'Two' })); await act(async () => settle(all));
  await screen.findByText('No Markdown indexed in this project yet.'); expect(screen.queryByText('Reported note')).toBeNull();
});
test('paused project remains browsable; unavailable project is disabled and errors retry without widening scope', async () => {
  const api = backend(); api.projects = vi.fn(async () => [{ id: source, name: 'Paused', enabled: true, monitoring: false }, { id: 'c'.repeat(64), name: 'Removed', enabled: false, monitoring: false }]);
  vi.mocked(api.documents).mockRejectedValueOnce(new Error('private config')).mockResolvedValue([]);
  render(<AgentProjects backend={api} onOpen={vi.fn()} onManage={vi.fn()} />); fireEvent.click(screen.getByRole('button', { name: 'Projects' }));
  fireEvent.click(await screen.findByRole('button', { name: /Paused/ }));
  expect((screen.getByRole('button', { name: /Removed/ }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(await screen.findByRole('button', { name: 'Retry' })); await screen.findByText('No Markdown indexed in this project yet.');
  expect(screen.queryByText('private config')).toBeNull(); expect(api.documents).toHaveBeenLastCalledWith(source, null);
});
