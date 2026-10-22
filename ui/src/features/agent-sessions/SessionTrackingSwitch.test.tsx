import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import type { AgentActivityBackend, AgentKind, AgentTrackingState } from './contracts';
import { SessionTrackingSwitch } from './SessionTrackingSwitch';

function fake(initial: AgentTrackingState) {
  let current = initial;
  return {
    list: vi.fn(), files: vi.fn(), rename: vi.fn(), hide: vi.fn(), loadOlder: vi.fn(), onChanged: vi.fn(),
    trackingState: vi.fn(async () => current),
    setTracking: vi.fn(async (agent: AgentKind, enabled: boolean) => (current = { ...current, [agent]: { ...current[agent], enabled } })),
    forget: vi.fn(async () => {}),
  } satisfies AgentActivityBackend;
}
const off: AgentTrackingState = { claude: { enabled: false, available: true }, codex: { enabled: false, available: false } };
afterEach(cleanup);

test('tracking is opt-in per client; turning it off offers a confirmed removal of recorded sessions', async () => {
  const api = fake(off);
  render(<SessionTrackingSwitch agent="claude" active backend={api} />);
  const toggle = await screen.findByRole('switch', { name: 'Track sessions' }) as HTMLInputElement;
  await waitFor(() => expect(toggle.disabled).toBe(false));
  expect(toggle.checked).toBe(false);
  fireEvent.click(toggle);
  await waitFor(() => expect(toggle.checked).toBe(true));
  expect(api.setTracking).toHaveBeenCalledWith('claude', true);
  expect(screen.queryByRole('button', { name: 'Remove tracked sessions' })).toBeNull();
  fireEvent.click(toggle);
  fireEvent.click(await screen.findByRole('button', { name: 'Remove tracked sessions' }));
  expect(api.forget).not.toHaveBeenCalled();
  fireEvent.click(screen.getByRole('button', { name: 'Remove' }));
  expect((await screen.findByRole('status')).textContent).toBe('Tracked Claude Code sessions were removed. Your files were not changed.');
  expect(api.forget).toHaveBeenCalledWith('claude');
});

test('a client without session logs cannot be turned on and says why', async () => {
  const api = fake(off);
  render(<SessionTrackingSwitch agent="codex" active backend={api} />);
  expect(await screen.findByText('No Codex session logs were found on this device.')).toBeTruthy();
  expect((screen.getByRole('switch', { name: 'Track sessions' }) as HTMLInputElement).disabled).toBe(true);
});

test('a failed change reports a fixed message', async () => {
  const api = fake(off); api.setTracking.mockRejectedValueOnce(new Error('/private/secret'));
  render(<SessionTrackingSwitch agent="claude" active backend={api} />);
  const toggle = await screen.findByRole('switch', { name: 'Track sessions' }) as HTMLInputElement;
  await waitFor(() => expect(toggle.disabled).toBe(false));
  fireEvent.click(toggle);
  expect((await screen.findByRole('alert')).textContent).toBe('Claude Code session tracking could not be changed.');
  expect(toggle.checked).toBe(false);
});

test('a status read queued before a change cannot undo the change', async () => {
  const api = fake(off);
  const view = render(<SessionTrackingSwitch agent="claude" active backend={api} />);
  const toggle = await screen.findByRole('switch', { name: 'Track sessions' }) as HTMLInputElement;
  await waitFor(() => expect(toggle.disabled).toBe(false));
  let stale!: (state: AgentTrackingState) => void;
  api.trackingState.mockImplementationOnce(() => new Promise(done => { stale = done; }));
  view.rerender(<SessionTrackingSwitch agent="claude" active={false} backend={api} />);
  view.rerender(<SessionTrackingSwitch agent="claude" active backend={api} />);
  fireEvent.click(toggle);
  await waitFor(() => expect(toggle.checked).toBe(true));
  await act(async () => stale(off));
  expect(toggle.checked).toBe(true);
});
