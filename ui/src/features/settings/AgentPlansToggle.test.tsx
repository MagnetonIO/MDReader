import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { AgentPlansToggle, type AgentPlansBackend } from './AgentPlansToggle';
afterEach(cleanup);
const PATH = '/public/home/.claude/plans';
test('enabling adds the plans folder and disabling forgets only that membership', async () => {
  const backend: AgentPlansBackend = {
    status: vi.fn(async () => ({ path: PATH, source_id: null })),
    enable: vi.fn(async () => ({ path: PATH, source_id: 'plans-source' })),
    disable: vi.fn(async () => undefined),
  };
  render(<AgentPlansToggle backend={backend} />);
  const toggle = screen.getByRole('switch', { name: 'Index Claude plans' }) as HTMLInputElement;
  await waitFor(() => expect(toggle.disabled).toBe(false));
  expect(screen.getByText('Adds your Claude Code plans folder as a read-only source.')).toBeTruthy();
  fireEvent.click(toggle);
  await waitFor(() => expect(toggle.checked).toBe(true));
  expect(backend.enable).toHaveBeenCalledOnce();
  fireEvent.click(toggle);
  await waitFor(() => expect(toggle.checked).toBe(false));
  expect(backend.disable).toHaveBeenCalledWith('plans-source');
});
test('a missing folder is reported and a project-relative setting cannot be toggled', async () => {
  const backend: AgentPlansBackend = {
    status: vi.fn(async () => ({ path: PATH, source_id: null })),
    enable: vi.fn(async () => { throw new Error('No Claude Code plans folder'); }),
    disable: vi.fn(),
  };
  const view = render(<AgentPlansToggle backend={backend} />);
  const toggle = screen.getByRole('switch', { name: 'Index Claude plans' }) as HTMLInputElement;
  await waitFor(() => expect(toggle.disabled).toBe(false));
  fireEvent.click(toggle);
  expect((await screen.findByRole('alert')).textContent).toBe('No Claude Code plans folder');
  expect(toggle.checked).toBe(false);
  view.unmount();
  render(<AgentPlansToggle backend={{ ...backend, status: async () => ({ path: null, source_id: null }) }} />);
  await waitFor(() => expect(backend.status).toHaveBeenCalled());
  expect((screen.getByRole('switch') as HTMLInputElement).disabled).toBe(true);
});
