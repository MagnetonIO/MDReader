import { commands } from '../../bindings';
import type { DeviceRequest, DeviceReply } from '../../bindings';
import type { ProviderDocumentBody } from '../../platform/providers';
import type { CanonicalReaderReference } from '../reader/contracts';
import type { AssetBackend } from '../../mobile/device-assets';
import type { AssetManifest } from '../../bindings';

export type DeviceReference = CanonicalReaderReference & { documentId: string };
export function isDeviceReference(reference: CanonicalReaderReference): reference is DeviceReference {
  return typeof reference.documentId === 'string';
}
export interface DeviceReaderBackend {
  read(reference: DeviceReference): Promise<ProviderDocumentBody & { documentId: string }>;
  manifest(reference: DeviceReference): Promise<AssetManifest | null>;
  images(reference: DeviceReference): AssetBackend;
}
function receipt(reference: DeviceReference) {
  if (reference.path !== 'document.md'
    || !/^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$/.test(reference.documentId)
    || !/^[a-f0-9]{64}$/.test(reference.sourceId) || !/^[a-f0-9]{64}$/.test(reference.revision)) {
    throw new Error('The selected device receipt is invalid.');
  }
  return { documentId: reference.documentId, sourceId: reference.sourceId, revision: reference.revision };
}
function message(kind: string) {
  if (kind === 'stale') return 'The device document changed. Reload On this device before opening it again.';
  if (kind === 'busy') return 'The device owner is busy. Try again after the current operation.';
  if (kind === 'unsupported') return 'This device namespace requires a supported reader version.';
  if (kind === 'unconfigured') return 'Device reading is unavailable in this native service.';
  return 'The exact device copy or selected image is unavailable.';
}
async function request(value: DeviceRequest): Promise<DeviceReply> {
  try {
    const result = await commands.readerDevice(value);
    if (result.status === 'error') throw new Error(message(result.error.kind));
    return result.data;
  } catch (cause) {
    throw cause instanceof Error ? cause : new Error('The device reader service is unavailable.');
  }
}
export const deviceReaderBackend: DeviceReaderBackend = {
  async read(reference) {
    const value = await request({ operation: 'read', receipt: receipt(reference) });
    if (value.operation !== 'read_document' || value.documentId !== reference.documentId
      || value.sourceId !== reference.sourceId || value.revision !== reference.revision
      || value.path !== 'document.md') throw new Error('The exact device receipt could not be verified.');
    return value;
  },
  async manifest(reference) {
    const value = await request({ operation: 'manifest', receipt: receipt(reference) });
    if (value.operation !== 'manifest' || value.value && (value.value.document_id !== reference.documentId
      || value.value.document_revision !== reference.revision)) throw new Error('The selected image manifest changed.');
    return value.value;
  },
  images(reference) {
    const selected = receipt(reference);
    return async (input) => {
      switch (input.operation) {
        case 'open': {
          if (input.document_id !== selected.documentId || input.revision !== selected.revision) throw new Error('The image document selection changed.');
          const reply = await request({ operation: 'open', receipt: selected, assetId: input.asset_id, manifest: input.manifest });
          if (reply.operation !== 'opened') throw new Error('The image lease is unavailable.');
          return { operation: 'opened', session_id: reply.sessionId, receipt: reply.receipt };
        }
        case 'chunk': {
          const reply = await request({ operation: 'chunk', sessionId: input.session_id, offset: input.offset });
          if (reply.operation !== 'chunk') throw new Error('The image chunk is unavailable.');
          return reply;
        }
        case 'close': {
          const reply = await request({ operation: 'close', sessionId: input.session_id });
          if (reply.operation !== 'complete') throw new Error('The image lease could not be closed.');
          return reply;
        }
        default: throw new Error('This reader supports selected image reads only.');
      }
    };
  },
};
