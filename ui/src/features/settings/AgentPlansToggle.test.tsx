import { cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { AgentPlansToggle, type AgentPlansBackend } from './AgentPlansToggle';
afterEach(cleanup);
const PATH = '/public/home/.claude/plans';
const OFF = { path: PATH, source_id: null, reader_source_id: null };
const ON = { path: PATH, source_id: 'plans-member', reader_source_id: 'plans-reader' };
const HINT = 'Grant agents access to Claude plans to link plans to sessions.';
test('enabling adds the plans folder and disabling forgets only that membership', async () => {
  const backend: AgentPlansBackend = {
    status: vi.fn(async () => OFF),
    enable: vi.fn(async () => ON),
    disable: vi.fn(async () => undefined),
    grantedSources: vi.fn(async () => ['plans-reader']),
  };
  render(<AgentPlansToggle backend={backend} />);
  const toggle = screen.getByRole('switch', { name: 'Index Claude plans' }) as HTMLInputElement;
  await waitFor(() => expect(toggle.disabled).toBe(false));
  expect(screen.getByText('Adds your Claude Code plans folder as a read-only source.')).toBeTruthy();
  fireEvent.click(toggle);
  await waitFor(() => expect(toggle.checked).toBe(true));
  expect(backend.enable).toHaveBeenCalledOnce();
  await waitFor(() => expect(backend.grantedSources).toHaveBeenCalled());
  expect(screen.queryByText(HINT)).toBeNull();
  fireEvent.click(toggle);
  await waitFor(() => expect(toggle.checked).toBe(false));
  expect(backend.disable).toHaveBeenCalledWith('plans-member');
});
test('an enabled plans source without an agent grant says how to link sessions', async () => {
  const backend: AgentPlansBackend = {
    status: vi.fn(async () => ON), enable: vi.fn(), disable: vi.fn(),
    grantedSources: vi.fn(async () => ['project-reader']),
  };
  render(<AgentPlansToggle backend={backend} />);
  expect(await screen.findByText(HINT)).toBeTruthy();
});
test('a folder already in the Library is refused and a project-relative setting cannot be toggled', async () => {
  const backend: AgentPlansBackend = {
    status: vi.fn(async () => OFF),
    enable: vi.fn(async () => { throw new Error('This folder is already in your Library, so it was left editable.'); }),
    disable: vi.fn(),
    grantedSources: vi.fn(async () => []),
  };
  const view = render(<AgentPlansToggle backend={backend} />);
  const toggle = screen.getByRole('switch', { name: 'Index Claude plans' }) as HTMLInputElement;
  await waitFor(() => expect(toggle.disabled).toBe(false));
  fireEvent.click(toggle);
  expect((await screen.findByRole('alert')).textContent).toBe('This folder is already in your Library, so it was left editable.');
  expect(toggle.checked).toBe(false);
  expect(backend.disable).not.toHaveBeenCalled();
  view.unmount();
  render(<AgentPlansToggle backend={{ ...backend, status: async () => ({ path: null, source_id: null, reader_source_id: null }) }} />);
  await waitFor(() => expect(backend.status).toHaveBeenCalled());
  expect((screen.getByRole('switch') as HTMLInputElement).disabled).toBe(true);
});
