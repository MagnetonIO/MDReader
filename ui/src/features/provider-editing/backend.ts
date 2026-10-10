import { invoke } from '@tauri-apps/api/core';
import { providerRequestQueue } from '../../platform/provider-request-queue';
import { NativeProviderError, type ProviderProjection, type ProviderReaderReference } from '../../platform/providers';

export type SaveState = 'absent' | 'prepared' | 'dispatched' | 'unknown' | 'accepted' | 'published' | 'conflict';
export interface ProviderEditTicket {
  operation: 'begin_edit_exact'; writeSourceId: string; editId: string;
  markdown: string; expiresAtMs: number; projection: ProviderProjection;
}
export interface SaveInspection {
  operation: 'inspect_save'; operationId: string; state: SaveState; projection: ProviderProjection | null;
}
async function request<T>(request: object): Promise<T> {
  try { return await providerRequestQueue.run('foreground', () => invoke<T>('native_provider_request', { request })); }
  catch (cause) {
    if (cause && typeof cause === 'object' && 'kind' in cause && typeof cause.kind === 'string')
      throw new NativeProviderError({ kind: cause.kind });
    throw new NativeProviderError({ kind: 'service_unavailable' });
  }
}
export function sameProjection(reference: ProviderReaderReference, projection: ProviderProjection): boolean {
  return projection.readerSourceId === reference.sourceId && projection.path === reference.path && projection.revision === reference.revision;
}
function validProjection(value: ProviderProjection, reference: ProviderReaderReference): boolean {
  return value.readerSourceId === reference.sourceId && value.path === reference.path && typeof value.revision === 'string' && value.revision.length > 0;
}
export const providerEditBackend = {
  async begin(reference: ProviderReaderReference): Promise<ProviderEditTicket> {
    const value = await request<ProviderEditTicket>({ operation: 'begin_edit_exact', sourceId: reference.sourceId, path: reference.path, revision: reference.revision });
    if (value.operation !== 'begin_edit_exact' || !sameProjection(reference, value.projection)
      || !value.editId || !value.writeSourceId || typeof value.markdown !== 'string'
      || new TextEncoder().encode(value.markdown).length > 2 * 1024 * 1024 || !Number.isFinite(value.expiresAtMs))
      throw new Error('The exact provider edit receipt could not be verified.');
    return value;
  },
  async prepare(writeSourceId: string, editId: string, markdown: string): Promise<void> {
    const value = await request<{ operation: 'prepare_save'; operationId: string; state: string }>({ operation: 'prepare_save', sourceId: writeSourceId, editId, markdown });
    if (value.operation !== 'prepare_save' || value.operationId !== editId || value.state !== 'prepared')
      throw new Error('The provider save receipt could not be verified. Inspect this save before continuing.');
  },
  async dispatch(writeSourceId: string, operationId: string, reference: ProviderReaderReference): Promise<ProviderProjection> {
    const value = await request<{ operation: 'dispatch_save'; projection: ProviderProjection }>({ operation: 'dispatch_save', sourceId: writeSourceId, operationId });
    if (value.operation !== 'dispatch_save' || !validProjection(value.projection, reference))
      throw new Error('The saved provider revision could not be verified. Inspect this save before continuing.');
    return value.projection;
  },
  async inspect(writeSourceId: string, operationId: string, reference: ProviderReaderReference): Promise<SaveInspection> {
    const value = await request<SaveInspection>({ operation: 'inspect_save', sourceId: writeSourceId, operationId });
    if (value.operation !== 'inspect_save' || value.operationId !== operationId
      || !['absent', 'prepared', 'dispatched', 'unknown', 'accepted', 'published', 'conflict'].includes(value.state)
      || value.projection !== null && !validProjection(value.projection, reference))
      throw new Error('The provider save state could not be verified. Your buffer was kept.');
    return value;
  },
  async repair(writeSourceId: string): Promise<void> {
    await request({ operation: 'repair', sourceId: writeSourceId });
  },
};
