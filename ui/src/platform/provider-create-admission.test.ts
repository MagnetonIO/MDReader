import { expect, test, vi } from 'vitest';
const raw = vi.hoisted(() => ({ invoke: vi.fn() }));
vi.mock('@tauri-apps/api/core', () => raw);
import { nativeProvidersBackend, ProviderRequestRetiredError } from './providers';

test('a queued cloud create rechecks its caller scope before native invocation', async () => {
  let finish!: (value: unknown) => void;
  raw.invoke.mockImplementationOnce(() => new Promise(resolve => { finish = resolve; }));
  const refresh = nativeProvidersBackend.refresh('public-source');
  let current = true;
  const creation = nativeProvidersBackend.createDocument!({ sourceId: 'public-source', operationId: 'a1111111-1111-4111-8111-111111111111', filename: 'Public.md', markdown: '' }, () => current);
  const rejected = expect(creation).rejects.toBeInstanceOf(ProviderRequestRetiredError);
  current = false;
  finish({ operation: 'refresh', pages: 1, metadataComplete: false });
  await refresh; await rejected;
  expect(raw.invoke).toHaveBeenCalledOnce();
  expect(raw.invoke).toHaveBeenCalledWith('native_provider_request', { request: { operation: 'refresh', sourceId: 'public-source' } });
});
