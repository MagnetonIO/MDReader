import { afterEach, expect, test, vi } from 'vitest';
import { commands } from '../../bindings';
import { CatalogError, catalogBackend } from './backend';
afterEach(() => vi.restoreAllMocks());
test('generated native contract preserves typed stale errors and rejects wrong reply operations', async () => {
  const call = vi.spyOn(commands, 'readerCatalog').mockResolvedValue({status:'error',error:{kind:'stale'}});
  await expect(catalogBackend.page('local:opaque', 'cursor', 100)).rejects.toMatchObject({kind:'stale'});
  expect(call).toHaveBeenCalledWith({operation:'page',catalogId:'local:opaque',cursor:'cursor',limit:100});
  call.mockResolvedValue({status:'ok',data:{operation:'complete'}});
  await expect(catalogBackend.list()).rejects.toBeInstanceOf(CatalogError);
});
test('exact native capture kind is retained and unknown read capabilities fail closed', async () => {
  const reference = { sourceId: 'a'.repeat(64), path: 'output.md', revision: 'b'.repeat(64) };
  const data = { operation: 'read_document' as const, ...reference, markdown: '# Plan', lineStart: 1, lineEnd: 1, sourceKind: 'capture' };
  const call = vi.spyOn(commands, 'readerCatalog').mockResolvedValue({ status: 'ok', data });
  expect(await catalogBackend.readDocument(reference)).toMatchObject({ ...reference, sourceKind: 'capture' });
  const unknown = { ...data, sourceKind: 'unknown-future' };
  call.mockResolvedValue({ status: 'ok', data: unknown });
  await expect(catalogBackend.readDocument(reference)).rejects.toMatchObject({ kind: 'unsupported' });
});
