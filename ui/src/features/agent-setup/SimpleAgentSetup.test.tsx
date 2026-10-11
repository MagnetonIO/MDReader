import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { SimpleAgentSetup } from './SimpleAgentSetup';
import type { AgentSetupState, SimpleAgentBackend } from '../../platform/agent-setup';
const state: AgentSetupState = {
  supported: true, installed: false, enabled: false, command: "'/Applications/MDReader.app/Contents/MacOS/mdreader-app' --mdreader-agent install --agent codex --scope user",
  client_version: '0.162.1', diagnostic: null, source_ids: [], sources: [{ id: 'source-one', name: 'Public notes' }, { id: 'source-two', name: 'Other notes' }],
  projects: [{ path: '/public/project', name: 'Project' }], connection_status: 'not_verified',
};
function backend(): SimpleAgentBackend {
  return { state: vi.fn(async () => state), install: vi.fn(async () => ({ ...state, installed: true })), setMcp: vi.fn<SimpleAgentBackend['setMcp']>(async (_selection, ids, enabled) => ({ ...state, installed: true, enabled, source_ids: ids })), uninstall: vi.fn(async () => state) };
}
function openOptions() {
  const details = screen.getByText('Advanced: installation options', { selector: 'summary' }).closest('details')!;
  details.open = true; fireEvent(details, new Event('toggle'));
}
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); });
test('install is explicit, defaults to all projects, and never enables MCP or preselects sources', async () => {
  const api = backend(); render(<SimpleAgentSetup root="/public/project" active backend={api} />);
  await screen.findByText('Not installed');
  expect(api.state).toHaveBeenCalledWith({ agent: 'codex', scope: 'user', project_root: null });
  expect(api.install).not.toHaveBeenCalled(); expect(api.setMcp).not.toHaveBeenCalled();
  expect(screen.queryByRole('combobox')).toBeNull();
  const options = screen.getByText('Advanced: installation options', { selector: 'summary' }).closest('details')!;
  expect(options.open).toBe(false);
  expect(options.contains(screen.getByLabelText('Terminal fallback'))).toBe(true);
  expect(screen.queryByRole('checkbox', { name: 'Public notes' })).toBeNull();
  fireEvent.click(screen.getByRole('button', { name: 'Install integration' }));
  await screen.findByText('Installed · MCP off');
  expect(api.install).toHaveBeenCalledOnce();
  expect(api.install).toHaveBeenCalledWith({ agent: 'codex', scope: 'user', project_root: null });
  expect((screen.getByRole('checkbox', { name: 'Public notes' }) as HTMLInputElement).checked).toBe(false);
  expect(api.setMcp).not.toHaveBeenCalled(); expect(screen.queryByText('Connected', { exact: true })).toBeNull();
});
test('copying the advanced installed-binary fallback never installs or grants sources', async () => {
  const api = backend(), writeText = vi.fn(async () => {});
  Object.defineProperty(navigator, 'clipboard', { configurable: true, value: { writeText } });
  render(<SimpleAgentSetup root={null} active backend={api} />);
  await screen.findByText('Not installed'); openOptions();
  fireEvent.click(screen.getByRole('button', { name: 'Copy command' }));
  await waitFor(() => expect(writeText).toHaveBeenCalledWith(state.command));
  expect(api.install).not.toHaveBeenCalled(); expect(api.setMcp).not.toHaveBeenCalled();
});
test('advanced one-project selection uses only a listed target; agent choices are labeled radios', async () => {
  const api = backend(); render(<SimpleAgentSetup root={null} active backend={api} />);
  await screen.findByText('Not installed'); openOptions();
  fireEvent.click(screen.getByRole('radio', { name: 'One project' }));
  await waitFor(() => expect(api.state).toHaveBeenLastCalledWith({ agent: 'codex', scope: 'project', project_root: null }));
  expect((screen.getByRole('button', { name: 'Install integration' }) as HTMLButtonElement).disabled).toBe(true);
  fireEvent.change(screen.getByRole('combobox', { name: 'Project' }), { target: { value: '/public/project' } });
  await waitFor(() => expect(api.state).toHaveBeenLastCalledWith({ agent: 'codex', scope: 'project', project_root: '/public/project' }));
  fireEvent.click(screen.getByRole('radio', { name: 'Claude Code' }));
  await waitFor(() => expect(api.state).toHaveBeenLastCalledWith({ agent: 'claude', scope: 'project', project_root: '/public/project' }));
  expect(api.install).not.toHaveBeenCalled();
});
test.each([{ client_version: null }, { supported: false }])('missing client or unsupported host cannot install: %j', async override => {
  const api = backend(); vi.mocked(api.state).mockResolvedValue({ ...state, ...override });
  render(<SimpleAgentSetup root={null} active backend={api} />);
  const install = await screen.findByRole('button', { name: 'Install integration' });
  expect((install as HTMLButtonElement).disabled).toBe(true); fireEvent.click(install);
  expect(api.install).not.toHaveBeenCalled();
});
test('late install completion cannot publish into a retired settings target', async () => {
  const api = backend(); let resolve!: (value: AgentSetupState) => void;
  vi.mocked(api.install).mockReturnValueOnce(new Promise(done => { resolve = done; }));
  const view = render(<SimpleAgentSetup root={null} active backend={api} />);
  await screen.findByText('Not installed'); fireEvent.click(screen.getByRole('button', { name: 'Install integration' }));
  view.rerender(<SimpleAgentSetup root={null} active={false} backend={api} />);
  await act(async () => resolve({ ...state, installed: true }));
  expect(screen.queryByText('Installed · MCP off')).toBeNull();
  expect(screen.queryByText('Integration installed. Choose sources to enable MCP.')).toBeNull();
  expect(api.setMcp).not.toHaveBeenCalled();
});
test('enable sends only explicit selected sources and disabling is a separate action', async () => {
  const api = backend(); vi.mocked(api.state).mockResolvedValue({ ...state, installed: true });
  render(<SimpleAgentSetup root={null} active backend={api} />);
  await screen.findByText('Installed · MCP off');
  fireEvent.click(screen.getByRole('checkbox', { name: 'Public notes' }));
  fireEvent.click(screen.getByRole('button', { name: 'Enable MCP' }));
  await screen.findByRole('button', { name: 'Disable MCP' });
  expect(api.setMcp).toHaveBeenCalledWith({ agent: 'codex', scope: 'user', project_root: null }, ['source-one'], true);
  fireEvent.click(screen.getByRole('button', { name: 'Disable MCP' }));
  await screen.findByRole('button', { name: 'Enable MCP' });
  expect(api.setMcp).toHaveBeenLastCalledWith({ agent: 'codex', scope: 'user', project_root: null }, [], false);
});
test('failed status refresh and a missing installation retain explicit live-grant revoke', async () => {
  const api = backend(); vi.mocked(api.state).mockRejectedValueOnce(new Error('Setup status unavailable'));
  vi.mocked(api.state).mockResolvedValue({ ...state, enabled: true, source_ids: ['source-one'] });
  render(<SimpleAgentSetup root={null} active backend={api} />);
  await screen.findByRole('alert'); fireEvent.click(screen.getByRole('button', { name: 'Refresh status' }));
  await screen.findByText('Configuration missing · MCP enabled');
  fireEvent.click(screen.getByRole('button', { name: 'Disable MCP' }));
  await waitFor(() => expect(api.setMcp).toHaveBeenCalledWith({ agent: 'codex', scope: 'user', project_root: null }, [], false));
  expect(api.install).not.toHaveBeenCalled();
});
