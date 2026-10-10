import { expect, test, vi } from 'vitest';
import { ProviderRequestQueue } from './provider-request-queue';

test('finishes one metadata boundary then runs explicit fetch/edit before the next batch without replay', async () => {
  const queue = new ProviderRequestQueue();
  const calls: string[] = [];
  let finish!: () => void;
  const batch = queue.run('metadata', () => new Promise<void>(resolve => { calls.push('batch1'); finish = resolve; }));
  const next = queue.run('metadata', async () => { calls.push('batch2'); });
  const read = queue.run('foreground', async () => { calls.push('fetch'); return 'exact-receipt'; });
  const dispatch = vi.fn(async () => { calls.push('dispatch'); throw new Error('unknown outcome'); });
  const save = queue.run('foreground', dispatch);
  const failure = expect(save).rejects.toThrow('unknown outcome');
  expect(calls).toEqual(['batch1']);
  finish();
  expect(await read).toBe('exact-receipt');
  await failure;
  await Promise.all([batch, next]);
  expect(calls).toEqual(['batch1', 'fetch', 'dispatch', 'batch2']);
  expect(dispatch).toHaveBeenCalledTimes(1);
});

test('bounds waiting requests and releases admission after failure', async () => {
  const queue = new ProviderRequestQueue();
  let finish!: () => void;
  const first = queue.run('metadata', () => new Promise<void>(resolve => { finish = resolve; }));
  const pending = Array.from({ length: 32 }, () => queue.run('foreground', async () => 1));
  const never = vi.fn();
  await expect(queue.run('foreground', never)).rejects.toMatchObject({ kind: 'busy' });
  expect(never).not.toHaveBeenCalled();
  finish(); await first; await Promise.all(pending);
  await expect(queue.run('foreground', async () => 2)).resolves.toBe(2);
});
