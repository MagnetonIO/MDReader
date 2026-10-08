import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import type { AgentCaptureBackend, AgentCaptureState } from '../platform/agents.js';
import { AgentCapturePanel } from './AgentCapturePanel.js';
afterEach(cleanup);
const state = (root: string): AgentCaptureState => ({
  root, enabled: false, capture_subagents: false, excluded_paths: [], excluded_sessions: [],
  pending_events: 0, admitted_occurrences: 0, diagnostic: null, supported: true,
});
test('opening settings and reading a policy never opts in or edits client configuration', async () => {
  const backend: AgentCaptureBackend = { state: vi.fn(async (root) => state(root)), update: vi.fn() };
  render(<AgentCapturePanel root="/project" backend={backend} integrationBackend={null} />);
  expect(backend.state).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Configure agent capture' }));
  await screen.findByRole('button', { name: 'Save capture settings' });
  expect((screen.getByRole('checkbox', { name: /Capture new/ }) as HTMLInputElement).checked).toBe(false);
  expect(backend.update).not.toHaveBeenCalled();
});
test('explicit opt-in saves only the selected folder, subagent choice and exclusions', async () => {
  const backend: AgentCaptureBackend = {
    state: vi.fn(async (root) => state(root)),
    update: vi.fn(async (root, update) => ({ ...state(root), ...update })),
  };
  render(<AgentCapturePanel root="/project" backend={backend} integrationBackend={null} />);
  fireEvent.click(screen.getByRole('button', { name: 'Configure agent capture' }));
  fireEvent.click(await screen.findByRole('checkbox', { name: /Capture new/ }));
  fireEvent.click(screen.getByRole('checkbox', { name: /Include subagent/ }));
  fireEvent.change(screen.getByRole('textbox', { name: /Excluded folders/ }), { target: { value: ' private\n\nvendor ' } });
  fireEvent.click(screen.getByRole('button', { name: 'Save capture settings' }));
  await waitFor(() => expect(backend.update).toHaveBeenCalledWith('/project', {
    enabled: true, capture_subagents: true, excluded_paths: ['private', 'vendor'], excluded_sessions: [],
  }));
  expect((await screen.findByRole('status')).textContent).toContain('must be installed');
});
test('a late response from another folder cannot show its policy or queue', async () => {
  let resolveOld!: (value: AgentCaptureState) => void;
  const backend: AgentCaptureBackend = {
    state: vi.fn((root) => root === '/old' ? new Promise<AgentCaptureState>((resolve) => { resolveOld = resolve; }) : Promise.resolve(state(root))),
    update: vi.fn(),
  };
  const { rerender } = render(<AgentCapturePanel root="/old" backend={backend} integrationBackend={null} />);
  fireEvent.click(screen.getByRole('button', { name: 'Configure agent capture' }));
  rerender(<AgentCapturePanel root="/new" backend={backend} integrationBackend={null} />);
  await screen.findByRole('button', { name: 'Save capture settings' });
  resolveOld({ ...state('/old'), enabled: true, admitted_occurrences: 999 });
  await waitFor(() => expect(screen.getByText(/Library capture queue/).textContent).toContain('0 captured'));
  expect((screen.getByRole('checkbox', { name: /Capture new/ }) as HTMLInputElement).checked).toBe(false);
});
test('an unsupported platform stays disabled', async () => {
  const backend: AgentCaptureBackend = { state: vi.fn(async (root) => ({ ...state(root), supported: false })), update: vi.fn() };
  render(<AgentCapturePanel root="/project" backend={backend} integrationBackend={null} />);
  fireEvent.click(screen.getByRole('button', { name: 'Configure agent capture' }));
  const button = await screen.findByRole('button', { name: 'Save capture settings' });
  expect((button.closest('fieldset') as HTMLFieldSetElement).disabled).toBe(true);
  expect(backend.update).not.toHaveBeenCalled();
});

test('a missing-root capture-state failure still permits explicit metadata-only backfill deselection', async () => {
  const backend: AgentCaptureBackend = { state: vi.fn(async () => { throw new Error('Selected root is unavailable'); }), update: vi.fn() };
  let selected = true;
  const request = vi.fn<import('../agents/backfill/backend').BackfillBackend['request']>(async (_root, input) => {
    if (input.operation === 'state') return { operation: 'state', enabled: false, selected_sessions: selected ? ['retained-session'] : [] };
    if (input.operation === 'deselect') { selected = false; return { operation: 'deselected', session_id: input.session_id }; }
    throw new Error('A missing root must never admit or review an output');
  });
  render(<AgentCapturePanel root="/missing" backend={backend} integrationBackend={null} backfillBackend={{ request }} />);
  fireEvent.click(screen.getByRole('button', { name: 'Configure agent capture' }));
  await screen.findByText('Selected root is unavailable');
  expect(request).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Import selected past output' }));
  fireEvent.click(await screen.findByRole('button', { name: 'Stop backfill for session retained-session' }));
  await waitFor(() => expect(screen.getByText(/Pending events remain retained/)).toBeTruthy());
  expect(request.mock.calls.map(([, input]) => input.operation)).toEqual(['state', 'deselect', 'state']);
  expect(backend.update).not.toHaveBeenCalled();
});
