import type { ProviderReadingRequest } from '../bindings';
import { readerBackend, unwrapNative } from '../platform/backend';
import type { ProviderReaderReference } from '../platform/providers';
import { transferCapabilities, type TransferCapabilities } from './document-transfer';
export interface ProviderReadingState {
  reference: { sourceId: string; path: string; revision: string };
  pinned: boolean; line: number; lineEnd: number; generation: string;
}
export interface ProviderDocumentActionsBackend {
  read(reference: ProviderReaderReference): Promise<ProviderReadingState>;
  update(reference: ProviderReaderReference, expectedGeneration: string, patch: { pinned?: boolean; line?: number }): Promise<ProviderReadingState>;
  capabilities(): Promise<TransferCapabilities>;
  transfer(reference: ProviderReaderReference, action: 'export' | 'share'): Promise<'exported' | 'presented' | 'cancelled'>;
}
export class ProviderDocumentActionError extends Error {
  constructor(readonly kind: string) {
    super(kind === 'stale' ? 'This cached revision or source is unavailable. Return to Sources to reconnect or explicitly open a current copy.'
      : kind === 'conflict' ? 'Your saved place changed in another operation. Refresh it before saving again.'
      : kind === 'busy' ? 'Another transfer is still in use. Close its system sheet before trying again.'
      : kind === 'unsupported' ? 'This build does not support this native document action.'
      : kind === 'limit' || kind === 'too_large' ? 'This action exceeds the supported native storage or copy limit.'
      : kind === 'schema' ? 'Saved reading state requires a supported reader version. Nothing was reset.'
      : 'The native action could not be confirmed. Your provider original is unchanged; check the selected destination before retrying.');
  }
}
function exact(reference: ProviderReaderReference) {
  if (!/^provider:[a-f0-9]{64}$/.test(reference.sourceId) || !/^records\/[a-f0-9]{64}\.md$/.test(reference.path) || !/^[a-f0-9]{64}$/.test(reference.revision)) throw new ProviderDocumentActionError('stale');
  return { sourceId: reference.sourceId, path: reference.path, revision: reference.revision };
}
function checked(value: ProviderReadingState, reference: ProviderReaderReference) {
  const selected = exact(reference);
  if (!value || !value.reference || value.reference.sourceId !== selected.sourceId || value.reference.path !== selected.path || value.reference.revision !== selected.revision
    || typeof value.pinned !== 'boolean' || !Number.isSafeInteger(value.line) || value.line < 1 || !Number.isSafeInteger(value.lineEnd) || value.lineEnd < value.line
    || typeof value.generation !== 'string' || !/^(?:0|[1-9][0-9]{0,18})$/.test(value.generation) || BigInt(value.generation) > 9223372036854775807n) throw new ProviderDocumentActionError('stale');
  return value;
}
async function state(reference: ProviderReaderReference, patch?: { pinned?: boolean; line?: number }, expectedGeneration?: string) {
  try {
    const request = (patch ? { operation: 'set_state', reference: exact(reference), expectedGeneration, pinned: patch.pinned ?? null, line: patch.line ?? null } : { operation: 'state', reference: exact(reference) }) as ProviderReadingRequest;
    return checked(await unwrapNative(readerBackend().mobile.mobileProviderReadingState(request)), reference);
  } catch (cause) { throw cause instanceof ProviderDocumentActionError ? cause : new ProviderDocumentActionError(typeof cause === 'string' ? cause : 'unavailable'); }
}
export const providerDocumentActionsBackend: ProviderDocumentActionsBackend = {
  read: reference => state(reference),
  update: (reference, expectedGeneration, patch) => state(reference, patch, expectedGeneration),
  capabilities: transferCapabilities,
  async transfer(reference, action) {
    try {
      const result = await unwrapNative(readerBackend().mobile.mobileProviderTransfer(exact(reference), action));
      if (result.state === 'cancelled' || action === 'export' && result.state === 'exported' || action === 'share' && result.state === 'presented') return result.state as 'exported' | 'presented' | 'cancelled';
      throw new ProviderDocumentActionError('unavailable');
    } catch (cause) { throw cause instanceof ProviderDocumentActionError ? cause : new ProviderDocumentActionError(typeof cause === 'string' ? cause : 'unavailable'); }
  },
};
