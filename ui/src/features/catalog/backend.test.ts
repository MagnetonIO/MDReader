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
