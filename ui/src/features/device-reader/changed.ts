import type { SelectedOperation } from '../account-sync/backend';

/** Invalidates display snapshots only; receivers re-read their native owner. */
export const DEVICE_READ_CHANGED = 'mdreader-device-read-changed';
export function notifyDeviceReadChanged(receipt: SelectedOperation['receipt']): void {
  window.dispatchEvent(new CustomEvent(DEVICE_READ_CHANGED, { detail: receipt }));
}
