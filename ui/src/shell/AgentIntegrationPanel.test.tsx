import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import type { AgentIntegrationBackend, AgentIntegrationReview, AgentIntegrationsState } from '../platform/agents.js';
import { AgentIntegrationPanel } from './AgentIntegrationPanel.js';
afterEach(cleanup);
const empty = (): AgentIntegrationsState => ({ installations: [], sources: [{ id: 'selected', name: 'Project notes' }] });
const review: AgentIntegrationReview = { preparationId: 'prepared', configPath: '/project/.mcp.json', policyPath: '/data/policy.json', beforeText: null, afterText: '{"safe":"<script>not executable</script>"}', capabilitiesVerified: false };
function backend(): AgentIntegrationBackend {
  return { state: vi.fn(async () => empty()), prepare: vi.fn(async () => review), apply: vi.fn(async () => {}), uninstall: vi.fn(async () => {}) };
}
async function chooseMcp() {
  fireEvent.change(screen.getByRole('combobox', { name: 'Integration' }), { target: { value: 'codex_mcp' } });
  fireEvent.change(screen.getByRole('textbox', { name: 'Installed agent version' }), { target: { value: 'explicit-client-version' } });
  fireEvent.click(await screen.findByRole('checkbox', { name: /Project notes/ }));
  fireEvent.click(screen.getByRole('checkbox', { name: /I trust this folder/ }));
}
test('capture hooks cannot be prepared before opt-in; MCP has explicit source and trust selection', async () => {
  const api = backend();
  render(<AgentIntegrationPanel root="/project" captureEnabled={false} backend={api} />);
  await waitFor(() => expect(api.state).toHaveBeenCalled());
  expect((screen.getByRole('button', { name: 'Review configuration change' }) as HTMLButtonElement).disabled).toBe(true);
  expect(api.prepare).not.toHaveBeenCalled();
  expect(api.apply).not.toHaveBeenCalled();
  await chooseMcp();
  fireEvent.click(screen.getByRole('button', { name: 'Review configuration change' }));
  await screen.findByRole('button', { name: 'Apply reviewed change' });
  expect(api.prepare).toHaveBeenCalledWith('/project', { kind: 'codex_mcp', client_version: 'explicit-client-version', compatibility_and_trust_confirmed: true, source_ids: ['selected'] });
  expect(api.apply).not.toHaveBeenCalled();
  expect(document.querySelector('script')).toBeNull();
});
test('only the explicit reviewed apply changes settings, then reports client trust as still required', async () => {
  const api = backend();
  render(<AgentIntegrationPanel root="/project" captureEnabled backend={api} />);
  await chooseMcp();
  fireEvent.click(screen.getByRole('button', { name: 'Review configuration change' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Apply reviewed change' }));
  await waitFor(() => expect(api.apply).toHaveBeenCalledWith('/project', 'prepared'));
  expect((await screen.findByRole('status')).textContent).toContain('Review and trust it in your agent');
});
test('a late review from a previous folder is discarded and never applied', async () => {
  const api = backend();
  let release!: (review: AgentIntegrationReview) => void;
  api.prepare = vi.fn(() => new Promise<AgentIntegrationReview>((resolve) => { release = resolve; }));
  const { rerender } = render(<AgentIntegrationPanel root="/project" captureEnabled backend={api} />);
  await chooseMcp();
  fireEvent.click(screen.getByRole('button', { name: 'Review configuration change' }));
  rerender(<AgentIntegrationPanel root="/other" captureEnabled backend={api} />);
  release(review);
  await waitFor(() => expect(api.state).toHaveBeenCalledWith('/other'));
  expect(screen.queryByRole('button', { name: 'Apply reviewed change' })).toBeNull();
  expect(api.apply).not.toHaveBeenCalled();
});
test('previously owned installations can be removed after reopening without displaying saved client settings', async () => {
  const api = backend();
  api.state = vi.fn(async () => ({ ...empty(), installations: [{ preparationId: 'owned', kind: 'codex_mcp', configPath: '/project/.codex/config.toml', state: 'applied', capabilitiesVerified: false }] }));
  render(<AgentIntegrationPanel root="/project" captureEnabled={false} backend={api} />);
  fireEvent.click(await screen.findByRole('button', { name: 'Remove integration' }));
  await waitFor(() => expect(api.uninstall).toHaveBeenCalledWith('/project', 'owned'));
  expect(api.prepare).not.toHaveBeenCalled();
  expect(screen.queryByText('Proposed configuration')).toBeNull();
});
