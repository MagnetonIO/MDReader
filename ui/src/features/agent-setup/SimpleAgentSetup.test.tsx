import { act, cleanup, fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { SimpleAgentSetup } from './SimpleAgentSetup';
import type { AgentSetupState, SimpleAgentBackend } from '../../platform/agent-setup';
import type { AgentActivityBackend } from '../agent-sessions/contracts';
import { StrictMode } from 'react';
const state: AgentSetupState = {
  supported: true, installed: false, enabled: false, command: null, client_version: '0.162.1', diagnostic: null,
  source_ids: [], edit_source_ids: [], reporting_project_ids: [], sources: [{ id: 'project-one', name: 'Public notes' }, { id: 'source-two', name: 'Other notes' }],
  projects: [{ id: 'project-one', path: '/public/project', name: 'Project' }], connection_status: 'not_verified', connected: false, last_seen_ms: null,
};
function backend(): SimpleAgentBackend {
  return { chooseProjectFolder: vi.fn(async () => null), state: vi.fn(async () => state), install: vi.fn(async () => ({ ...state, installed: true })),
    setPermissions: vi.fn(async (_selection, grants) => ({ ...state, installed: true, enabled: grants.enabled, source_ids: grants.read_source_ids, reporting_project_ids: grants.project_ids, edit_source_ids: grants.edit_source_ids })),
    setMcp: vi.fn(), uninstall: vi.fn(async () => state) };
}
const tracking = (): AgentActivityBackend => ({
  list: async () => ({ sessions: [], nextOffset: null }), files: async () => [], rename: vi.fn(), hide: vi.fn(), loadOlder: vi.fn(), forget: vi.fn(),
  trackingState: async () => ({ claude: { enabled: false, available: true }, codex: { enabled: false, available: true } }),
  setTracking: vi.fn(), onChanged: async () => () => {},
});
const setup = (api: SimpleAgentBackend, active = true) => <SimpleAgentSetup root={null} active={active} backend={api} trackingBackend={trackingBackend} />;
let trackingBackend = tracking();
const card = (name = 'Codex') => within(screen.getByRole('article', { name: `${name} connection` }));
function openDetails(name: string, title: string) { const details = card(name).getByText(title, { selector: 'summary' }).closest('details')!; details.open = true; fireEvent(details, new Event('toggle')); return details; }
afterEach(() => { cleanup(); vi.restoreAllMocks(); vi.unstubAllGlobals(); trackingBackend = tracking(); });
test('Agent access installs and enables MCP without project grants; read and edit start off', async () => {
  const api = backend(); render(setup(api));
  await waitFor(() => expect(card().getByText('Not connected')).toBeTruthy());
  expect(api.state).toHaveBeenCalledWith({ agent: 'codex', scope: 'user', project_root: null });
  expect(api.state).toHaveBeenCalledWith({ agent: 'claude', scope: 'user', project_root: null });
  expect(api.install).not.toHaveBeenCalled(); expect(api.setPermissions).not.toHaveBeenCalled();
  expect(screen.queryByText('Projects to organize')).toBeNull();
  fireEvent.click(card().getByRole('switch', { name: 'Agent access (MCP)' }));
  await waitFor(() => expect(api.setPermissions).toHaveBeenCalledWith({ agent: 'codex', scope: 'user', project_root: null }, { enabled: true, project_ids: [], read_source_ids: [], edit_source_ids: [] }));
  expect(api.install).toHaveBeenCalledOnce(); expect(api.setMcp).not.toHaveBeenCalled();
  expect(card().getByText('Waiting for agent')).toBeTruthy(); expect(screen.queryByText('Connected', { exact: true })).toBeNull();
  expect(card('Claude Code').getByText('Not connected')).toBeTruthy();
});
test('Connected requires handshake; access can be turned off without uninstalling', async () => {
  const api = backend(); vi.mocked(api.state).mockResolvedValue({ ...state, installed: true, enabled: true, connected: true, last_seen_ms: 1770000000000, reporting_project_ids: ['project-one'] });
  render(setup(api));
  await waitFor(() => expect(card().getByText('Connected')).toBeTruthy());
  expect(card().getByText(/Last seen/)).toBeTruthy();
  fireEvent.click(card().getByRole('switch', { name: 'Agent access (MCP)' }));
  await waitFor(() => expect(api.setPermissions).toHaveBeenCalledWith({ agent: 'codex', scope: 'user', project_root: null }, { enabled: false, project_ids: [], read_source_ids: [], edit_source_ids: [] }));
  expect(api.uninstall).not.toHaveBeenCalled();
});
test('optional read and create/edit permissions are independent explicit choices', async () => {
  const api = backend(); vi.mocked(api.state).mockResolvedValue({ ...state, installed: true });
  render(setup(api));
  await waitFor(() => expect(card().getByText('Installed · Access off')).toBeTruthy());
  openDetails('Codex', 'Advanced: source access');
  const permissions = card().getByRole('group', { name: 'Codex document permissions' });
  const first = permissions.querySelector('.agent-connection-card__permission')!;
  fireEvent.click(within(first as HTMLElement).getByRole('checkbox', { name: 'Create/edit' }));
  expect((within(first as HTMLElement).getByRole('checkbox', { name: 'Read' }) as HTMLInputElement).checked).toBe(false);
  fireEvent.click(card().getByRole('switch', { name: 'Agent access (MCP)' }));
  await waitFor(() => expect(api.setPermissions).toHaveBeenCalledWith(expect.anything(), { enabled: true, project_ids: [], read_source_ids: [], edit_source_ids: ['project-one'] }));
});
test('read-only capture sources cannot be enabled for creation/editing', async () => {
  const api = backend(); vi.mocked(api.state).mockResolvedValue({ ...state, installed: true, sources: [{ id: 'capture-source', name: 'Agent captures', editable: false }] });
  render(setup(api));
  await waitFor(() => expect(card().getByText('Installed · Access off')).toBeTruthy());
  openDetails('Codex', 'Advanced: source access');
  const permissions = card().getByRole('group', { name: 'Codex document permissions' });
  const edit = within(permissions).getByRole('checkbox', { name: 'Create/edit' });
  expect((edit as HTMLInputElement).disabled).toBe(true); fireEvent.click(edit);
  fireEvent.click(within(permissions).getByRole('checkbox', { name: 'Read' }));
  fireEvent.click(card().getByRole('switch', { name: 'Agent access (MCP)' }));
  await waitFor(() => expect(api.setPermissions).toHaveBeenCalledWith(expect.anything(), { enabled: true, project_ids: [], read_source_ids: ['capture-source'], edit_source_ids: [] }));
});
test('legacy reporting projects are not shown and saving access clears them', async () => {
  const api = backend(); vi.mocked(api.state).mockResolvedValue({ ...state, installed: true, reporting_project_ids: ['project-one'], projects: [{ id: 'project-one', path: '/public/project', name: 'Project', enabled: true, monitoring: false }] });
  render(setup(api));
  await waitFor(() => expect(card().getByText('Installed · Access off')).toBeTruthy());
  expect(card().queryByRole('checkbox', { name: /Project/ })).toBeNull();
  fireEvent.click(card().getByRole('switch', { name: 'Agent access (MCP)' }));
  await waitFor(() => expect(api.setPermissions).toHaveBeenCalledWith(expect.anything(), { enabled: true, project_ids: [], read_source_ids: [], edit_source_ids: [] }));
});
test('Advanced shows only portable public command and real picker is available for one-project scope', async () => {
  const api = backend(); const command = 'npx skills add MagnetonIO/MDReader --skill mdreader --agent codex --copy --global';
  vi.mocked(api.state).mockImplementation(async selection => ({ ...state, command: selection.agent === 'codex' ? command : "'/private/qa/build' --install" }));
  const writeText = vi.fn(async () => {}); vi.stubGlobal('navigator', { clipboard: { writeText } });
  render(setup(api));
  await waitFor(() => expect(card().getByText('Not connected')).toBeTruthy());
  const details = openDetails('Codex', 'Advanced: installation options');
  expect(card().getByText(command).closest('details')).toBe(details);
  fireEvent.click(card().getByRole('button', { name: 'Copy command' })); await waitFor(() => expect(writeText).toHaveBeenCalledWith(command));
  expect(screen.queryByText("'/private/qa/build' --install")).toBeNull();
  fireEvent.click(card().getByRole('radio', { name: 'One project' }));
  await waitFor(() => expect(api.state).toHaveBeenCalledWith({ agent: 'codex', scope: 'project', project_root: null }));
  expect(card().getByRole('button', { name: 'Choose project folder' })).toBeTruthy();
  expect(api.install).not.toHaveBeenCalled(); expect(api.setPermissions).not.toHaveBeenCalled();
});
test('retired Connect completion cannot enable MCP or publish into another settings target', async () => {
  const api = backend(); let resolve!: (value: AgentSetupState) => void;
  vi.mocked(api.install).mockReturnValueOnce(new Promise(done => { resolve = done; }));
  const view = render(setup(api));
  await waitFor(() => expect(card().getByText('Not connected')).toBeTruthy());
  fireEvent.click(card().getByRole('switch', { name: 'Agent access (MCP)' }));
  view.rerender(setup(api, false));
  await act(async () => resolve({ ...state, installed: true }));
  expect(api.setPermissions).not.toHaveBeenCalled(); expect(screen.queryByText('Installed · Access off')).toBeNull();
});
test('Strict Mode startup resolves both cards; unmounted Connect never enables a service afterward', async () => {
  const api = backend(); let resolve!: (value: AgentSetupState) => void;
  vi.mocked(api.install).mockReturnValueOnce(new Promise(done => { resolve = done; }));
  const view = render(<StrictMode>{setup(api)}</StrictMode>);
  await waitFor(() => expect(card().getByText('Not connected')).toBeTruthy());
  fireEvent.click(card().getByRole('switch', { name: 'Agent access (MCP)' }));
  view.unmount(); await act(async () => resolve({ ...state, installed: true }));
  expect(api.setPermissions).not.toHaveBeenCalled();
});
test.each([
  ['Invalid request: Agent integration conflicts with existing settings. Existing files were kept.', 'Agent integration conflicts with existing settings. Existing files were kept.'],
  ['I/O: /private/secret/config token=secret', 'Agent setup could not be completed. Existing files were kept. Refresh status and try again.'],
])('Claude rejection is safe and useful: %s', async (rejection, message) => {
  const api = backend(); vi.mocked(api.install).mockRejectedValue(rejection);
  render(setup(api));
  await waitFor(() => expect(card('Claude Code').getByText('Not connected')).toBeTruthy());
  fireEvent.click(card('Claude Code').getByRole('switch', { name: 'Agent access (MCP)' }));
  expect((await card('Claude Code').findByRole('alert')).textContent).toBe(message); expect(api.setPermissions).not.toHaveBeenCalled();
});
