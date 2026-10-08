import { invoke } from '@tauri-apps/api/core';
import { deviceErrorMessage, deviceReceipt, type DeviceReference } from './backend';

export interface DeviceReadingState {
  receipt: ReturnType<typeof deviceReceipt>;
  pinned: boolean;
  line: number;
  lineEnd: number;
  generation: string;
}
export interface DeviceReadingStateBackend {
  read(reference: DeviceReference): Promise<DeviceReadingState>;
  update(reference: DeviceReference, expectedGeneration: string, patch: { pinned?: boolean; line?: number }): Promise<DeviceReadingState>;
}
type StateOperation =
  | { operation: 'state'; receipt: DeviceReadingState['receipt'] }
  | { operation: 'set_state'; receipt: DeviceReadingState['receipt']; expectedGeneration: string; pinned?: boolean; line?: number };

async function request(operation: StateOperation): Promise<DeviceReadingState> {
  try {
    // This narrow projection follows the backend's agreed state/set_state
    // contract. The combined native generator will emit its canonical DTOs.
    const result = await invoke<DeviceReadingState & { operation: 'state' }>('reader_device', { operation });
    const exact = operation.receipt;
    const generation = Number(result.generation);
    if (result.operation !== 'state' || result.receipt.documentId !== exact.documentId
      || result.receipt.sourceId !== exact.sourceId || result.receipt.revision !== exact.revision
      || typeof result.pinned !== 'boolean' || !Number.isSafeInteger(result.line) || result.line < 1
      || !Number.isSafeInteger(result.lineEnd) || result.lineEnd < result.line
      || !Number.isSafeInteger(generation) || generation < 0 || String(generation) !== result.generation) {
      throw new Error('The exact document reading state could not be verified.');
    }
    return result;
  } catch (cause) {
    if (cause instanceof Error) throw cause;
    const kind = cause && typeof cause === 'object' && 'kind' in cause ? String(cause.kind) : 'unavailable';
    throw new Error(kind === 'conflict'
      ? 'The pin or reading position changed in another window. Refresh its status before saving again.'
      : deviceErrorMessage(kind));
  }
}
export const deviceReadingStateBackend: DeviceReadingStateBackend = {
  read: (reference) => request({ operation: 'state', receipt: deviceReceipt(reference) }),
  update: (reference, expectedGeneration, patch) => request({
    operation: 'set_state', receipt: deviceReceipt(reference), expectedGeneration, ...patch,
  }),
};
