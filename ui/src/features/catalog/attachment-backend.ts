import { commands } from '../../bindings';
import type { ProviderReaderReference } from '../../platform/providers';
import { assetErrorMessage, checkedAssetPreview } from '../reader/attachment-preview';

export async function readCatalogAttachment(reference: ProviderReaderReference, target: string) {
  const response = await commands.readerCatalog({ operation: 'attachment',
    sourceId: reference.sourceId, path: reference.path, revision: reference.revision, target });
  if (response.status === 'error') throw new Error(assetErrorMessage(response.error.kind));
  const reply = response.data;
  if (reply.operation !== 'attachment' || reply.sourceId !== reference.sourceId || reply.path !== reference.path
    || reply.revision !== reference.revision || reply.target !== target) {
    throw new Error('The linked file response could not be verified.');
  }
  return checkedAssetPreview(reply.preview);
}
