import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { SimpleAgentSetup } from './SimpleAgentSetup';
import type { AgentSetupState, SimpleAgentBackend } from '../../platform/agent-setup';
const state: AgentSetupState = {
  supported: true, installed: false, enabled: false, command: "'/Applications/MDReader.app/Contents/MacOS/mdreader-app' --mdreader-agent install --agent codex --scope project --project '/public/project'",
  client_version: '0.162.1', diagnostic: null, source_ids: [], sources: [{ id: 'source-one', name: 'Public notes' }, { id: 'source-two', name: 'Other notes' }],
  projects: [{ path: '/public/project', name: 'Project' }], connection_status: 'not_verified',
};
function backend(): SimpleAgentBackend {
  return { state: vi.fn(async () => state), setMcp: vi.fn<SimpleAgentBackend['setMcp']>(async (_selection, ids, enabled) => ({ ...state, installed: true, enabled, source_ids: ids })), uninstall: vi.fn(async () => state) };
}
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
test('copying the installed-binary setup command never installs, enables MCP or selects sources', async () => {
  const api = backend(), writeText = vi.fn(async () => {});
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
  render(<SimpleAgentSetup root="/public/project" active backend={api} />);
  await screen.findByText(/Not installed/);
  expect((screen.getByRole('button', { name: 'Enable MCP' }) as HTMLButtonElement).disabled).toBe(true);
  expect((screen.getByRole('checkbox', { name: 'Public notes' }) as HTMLInputElement).checked).toBe(false);
  fireEvent.click(screen.getByRole('button', { name: 'Copy command' }));
  await waitFor(() => expect(writeText).toHaveBeenCalledWith(state.command));
  expect(api.setMcp).not.toHaveBeenCalled(); expect(api.uninstall).not.toHaveBeenCalled();
  expect(screen.queryByText('Connected', { exact: true })).toBeNull();
});
test('enable sends only selected sources and disabling is an explicit separate action', async () => {
  const api = backend(); vi.mocked(api.state).mockResolvedValue({ ...state, installed: true });
  render(<SimpleAgentSetup root="/public/project" active backend={api} />);
  await screen.findByText(/Installed · MCP off/);
  fireEvent.click(screen.getByRole('checkbox', { name: 'Public notes' }));
  fireEvent.click(screen.getByRole('button', { name: 'Enable MCP' }));
  await screen.findByRole('button', { name: 'Disable MCP' });
  expect(api.setMcp).toHaveBeenCalledWith({ agent: 'codex', scope: 'project', project_root: '/public/project' }, ['source-one'], true);
  expect(screen.getByText(/Connection has not been verified/)).toBeTruthy();
  fireEvent.click(screen.getByRole('button', { name: 'Disable MCP' }));
  await screen.findByRole('button', { name: 'Enable MCP' });
  expect(api.setMcp).toHaveBeenLastCalledWith({ agent: 'codex', scope: 'project', project_root: '/public/project' }, [], false);
});
test('a late project status cannot become the user-scope configuration', async () => {
  const api = backend(); let resolve!: (value: AgentSetupState) => void;
  vi.mocked(api.state).mockReturnValueOnce(new Promise(done => { resolve = done; }));
  const view = render(<SimpleAgentSetup root="/public/project" active={false} backend={api} />);
  expect(api.state).not.toHaveBeenCalled();
  view.rerender(<SimpleAgentSetup root="/public/project" active backend={api} />);
  await waitFor(() => expect(api.state).toHaveBeenCalledOnce());
  // Scope is retired by host re-composition, not by bypassing the disabled controls.
  view.rerender(<SimpleAgentSetup root="/public/project" active={false} backend={api} />);
  await act(async () => resolve({ ...state, installed: true, enabled: true }));
  expect(screen.queryByText('MCP enabled', { exact: true })).toBeNull();
  view.rerender(<SimpleAgentSetup root="/public/project" active backend={api} />);
  await screen.findByText(/Not installed/);
  fireEvent.change(screen.getByRole('combobox', { name: 'Install for' }), { target: { value: 'user' } });
  await waitFor(() => expect(api.state).toHaveBeenLastCalledWith({ agent: 'codex', scope: 'user', project_root: null }));
});
test('a failed status can be refreshed without changing the agent configuration', async () => {
  const api = backend(); vi.mocked(api.state).mockRejectedValueOnce(new Error('Setup status unavailable'));
  render(<SimpleAgentSetup root={null} active backend={api} />);
  await screen.findByRole('alert');
  fireEvent.click(screen.getByRole('button', { name: 'Refresh status' }));
  await screen.findByText(/Not installed/);
  expect(api.setMcp).not.toHaveBeenCalled();
});
test('a missing installation still exposes the live grant and its revoke action', async () => {
  const api = backend();
  vi.mocked(api.state).mockResolvedValue({ ...state, installed: false, enabled: true, source_ids: ['source-one'] });
  render(<SimpleAgentSetup root="/public/project" active backend={api} />);
  await screen.findByText(/Configuration missing · MCP enabled/);
  fireEvent.click(screen.getByRole('button', { name: 'Disable MCP' }));
  await waitFor(() => expect(api.setMcp).toHaveBeenCalledWith({ agent: 'codex', scope: 'project', project_root: '/public/project' }, [], false));
});
