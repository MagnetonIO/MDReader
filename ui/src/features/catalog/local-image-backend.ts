import { commands } from '../../bindings';
import type { ProviderReaderReference } from '../../platform/providers';
import type { LocalImageBytes } from '../reader/contracts';

/** Native membership and retained handles own all path resolution. */
export async function readCatalogImage(reference: ProviderReaderReference, target: string): Promise<LocalImageBytes> {
  try {
    const response = await commands.readerCatalog({
      operation: 'image', sourceId: reference.sourceId, path: reference.path,
      revision: reference.revision, target,
    });
    if (response.status === 'error') throw new Error('Local image read was refused.');
    const value = response.data;
    if (!value || value.operation !== 'image' || value.sourceId !== reference.sourceId
      || value.path !== reference.path || value.revision !== reference.revision || value.target !== target
      || !['image/png', 'image/jpeg'].includes(value.mime) || typeof value.base64 !== 'string'
      || !value.base64.length || value.base64.length > 5_592_408) {
      throw new Error('Invalid local image receipt.');
    }
    return { mime: value.mime, base64: value.base64 };
  } catch {
    throw new Error('This local image is unavailable, unsupported, or its source changed. Your document is unchanged.');
  }
}
