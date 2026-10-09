import { afterEach, expect, test, vi } from 'vitest';
import { DeviceReaderFailure } from '../device-reader/backend';
import { readExact } from './exact-read';

afterEach(() => vi.useRealTimers());
const busy = (cause: unknown) => cause instanceof DeviceReaderFailure && cause.kind === 'busy';
test('exact native Busy reads retry automatically within the bounded delay', async () => {
  vi.useFakeTimers();
  const read = vi.fn().mockRejectedValueOnce(new DeviceReaderFailure('busy')).mockResolvedValue('exact revision');
  const waiting = vi.fn();
  const task = readExact(read, new AbortController().signal, busy, waiting);
  await vi.advanceTimersByTimeAsync(100);
  expect(await task).toBe('exact revision');
  expect(read).toHaveBeenCalledTimes(2); expect(waiting).toHaveBeenCalledOnce();
});
test('permanent Busy stops after six exact attempts and returns the known error for explicit Retry', async () => {
  vi.useFakeTimers();
  const failure = new DeviceReaderFailure('busy');
  const read = vi.fn().mockRejectedValue(failure);
  const failed = expect(readExact(read, new AbortController().signal, busy)).rejects.toBe(failure);
  await vi.runAllTimersAsync(); await failed;
  expect(read).toHaveBeenCalledTimes(6); expect(vi.getTimerCount()).toBe(0);
});
test('cancelling a selection during backoff retires its timer without issuing another read', async () => {
  vi.useFakeTimers();
  const cancelled = new AbortController();
  const read = vi.fn().mockRejectedValue(new DeviceReaderFailure('busy'));
  const task = readExact(read, cancelled.signal, busy);
  const failed = expect(task).rejects.toMatchObject({ name: 'AbortError' });
  await vi.advanceTimersByTimeAsync(0);
  cancelled.abort();
  await vi.runAllTimersAsync(); await failed;
  expect(read).toHaveBeenCalledOnce(); expect(vi.getTimerCount()).toBe(0);
});
test('other device errors and unrecognized errors never receive automatic retries', async () => {
  for (const failure of [new DeviceReaderFailure('stale'), new DeviceReaderFailure('unavailable'), new Error('busy')]) {
    const read = vi.fn().mockRejectedValue(failure);
    await expect(readExact(read, new AbortController().signal, busy)).rejects.toBe(failure);
    expect(read).toHaveBeenCalledOnce();
  }
});
