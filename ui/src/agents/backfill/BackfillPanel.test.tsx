import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, expect, test, vi } from 'vitest';
import { BackfillPanel } from './BackfillPanel';
import { selectedEvent, type AgentBackfillReply, type BackfillBackend } from './backend';
afterEach(cleanup);
const canonical = JSON.stringify({ cwd: '/project', hook_event_name: 'Stop', last_assistant_message: '<script>Visible only</script>', session_id: 'chosen' });
const reviewed: AgentBackfillReply = { operation: 'reviewed', value: { canonical_event: canonical, review_digest: 'd'.repeat(64),
  session_id: 'chosen', turn_id: null, turn_id_kind: null, subagent_id: null, cwd: '/project', body: '<script>Visible only</script>', selected: false } };
const file = (text: string): File => { const bytes = new TextEncoder().encode(text); return { size: bytes.length, arrayBuffer: async () => bytes.buffer } as File; };
async function expand() { fireEvent.click(screen.getByRole('button', { name: 'Import selected past output' })); await screen.findByLabelText('Choose one final-output JSON event'); }
test('one explicit output is reviewed, sanitized, and safely retried after an uncertain enqueue reply', async () => {
  let enqueues = 0; let selected = false;
  const request = vi.fn<BackfillBackend['request']>(async (_root, input) => {
    if (input.operation === 'state') return { operation: 'state', enabled: true, selected_sessions: selected ? ['chosen'] : [] };
    if (input.operation === 'review') return reviewed;
    if (input.operation === 'enqueue') {
      selected = true;
      if (++enqueues === 1) throw new Error('Queue reply lost; retry same event');
      return { operation: 'enqueued', outcome: { status: 'duplicate', id: 'existing' } };
    }
    throw new Error('No implicit policy or client configuration change is permitted');
  });
  const backend = { request };
  render(<BackfillPanel root="/project" captureEnabled backend={backend} />); expect(request).not.toHaveBeenCalled();
  await expand(); await waitFor(() => expect((screen.getByLabelText('Choose one final-output JSON event') as HTMLInputElement).disabled).toBe(false));
  fireEvent.change(screen.getByLabelText('Choose one final-output JSON event'), { target: { files: [file(JSON.stringify({ cwd: '/project', hook_event_name: 'Stop', last_assistant_message: '<script>Visible only</script>', session_id: 'chosen', reasoning: 'HIDDEN-SECRET' }))] } });
  await screen.findByText('Unavailable — no turn identifier will be invented');
  expect(screen.queryByText(/HIDDEN-SECRET/)).toBeNull(); expect(document.querySelector('script')).toBeNull();
  expect(screen.getByText('<script>Visible only</script>', { selector: 'pre' })).toBeTruthy();
  const queue = screen.getByRole('button', { name: 'Select this session and queue this output' }); expect((queue as HTMLButtonElement).disabled).toBe(true);
  fireEvent.click(screen.getByRole('checkbox')); fireEvent.click(queue);
  expect((await screen.findByRole('alert')).textContent).toContain('Queue reply lost');
  await waitFor(() => expect((queue as HTMLButtonElement).disabled).toBe(false)); fireEvent.click(queue);
  await waitFor(() => expect(screen.getByText(/No duplicate authorship was added/)).toBeTruthy());
  const sent = request.mock.calls.filter(([, input]) => input.operation === 'enqueue'); expect(sent).toHaveLength(2);
  for (const [root, input] of sent) expect([root, input]).toEqual(['/project', { operation: 'enqueue', adapter: 'claude_code', event: canonical, review_digest: 'd'.repeat(64), session_id: 'chosen' }]);
});
test('a delayed selected-file read cannot review or queue data in a different folder', async () => {
  let resolve!: (bytes: ArrayBuffer) => void; const bytes = new TextEncoder().encode('old event');
  const request = vi.fn<BackfillBackend['request']>(async () => ({ operation: 'state', enabled: true, selected_sessions: [] }));
  const backend = { request }; const view = render(<BackfillPanel root="/old" captureEnabled backend={backend} />);
  await expand(); await waitFor(() => expect((screen.getByLabelText('Choose one final-output JSON event') as HTMLInputElement).disabled).toBe(false));
  const delayed = { size: bytes.length, arrayBuffer: () => new Promise<ArrayBuffer>((done) => { resolve = done; }) } as File;
  fireEvent.change(screen.getByLabelText('Choose one final-output JSON event'), { target: { files: [delayed] } });
  view.rerender(<BackfillPanel root="/new" captureEnabled backend={backend} />);
  await act(async () => { resolve(bytes.buffer); });
  expect(request.mock.calls.every(([, input]) => input.operation === 'state')).toBe(true);
  expect(screen.queryByRole('checkbox')).toBeNull();
});
test('paused backfill selection can be explicitly removed without queueing or changing live capture', async () => {
  let selected = true;
  const request = vi.fn<BackfillBackend['request']>(async (_root, input) => {
    if (input.operation === 'state') return { operation: 'state', enabled: false, selected_sessions: selected ? ['old-session'] : [] };
    if (input.operation === 'deselect') { selected = false; return { operation: 'deselected', session_id: input.session_id }; }
    throw new Error('No output read or admission is authorized');
  });
  render(<BackfillPanel root="/project" captureEnabled={false} backend={{ request }} />); await expand();
  fireEvent.click(await screen.findByRole('button', { name: 'Stop backfill for session old-session' }));
  await waitFor(() => expect(screen.getByText(/Pending events remain retained/)).toBeTruthy());
  expect(request.mock.calls.map(([, input]) => input.operation)).toEqual(['state', 'deselect', 'state']);
});
test('selected events are byte bounded and invalid UTF-8 is rejected without replacement text', async () => {
  await expect(selectedEvent({ size: 256 * 1024 + 1 } as File)).rejects.toThrow('256 KiB');
  await expect(selectedEvent({ size: 1, arrayBuffer: async () => new Uint8Array([255]).buffer } as File)).rejects.toThrow('valid UTF-8');
});
