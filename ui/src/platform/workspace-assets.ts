import { commands, type WorkspaceOriginReceipt } from '../bindings';
import { assetErrorMessage, checkedAssetPreview, checkedRasterBytes } from '../features/reader/attachment-preview';
import type { LocalImageBytes } from '../features/reader/contracts';

export async function readWorkspaceAsset(origin: WorkspaceOriginReceipt, target: string, kind: 'image' | 'attachment') {
  const response = await commands.readerWorkspaceAsset({ origin, target, kind });
  if (response.status === 'error') throw new Error(assetErrorMessage(response.error.kind));
  return checkedAssetPreview(response.data);
}

export async function readWorkspaceImage(origin: WorkspaceOriginReceipt | undefined, target: string): Promise<LocalImageBytes> {
  if (!origin) throw new Error('This document has no current folder receipt. Reload it to load images.');
  const preview = await readWorkspaceAsset(origin, target, 'image');
  return checkedRasterBytes(preview);
}
