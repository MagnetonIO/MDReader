import type { PingResponse } from '../bindings';
import { readerBackend } from './backend';

/**
 * Thin wrapper over the backend's `ping` command. It exists to prove the
 * JS -> Rust -> JS round-trip (and the tauri-specta type flow) works.
 */
export async function pingBackend(msg: string): Promise<PingResponse> {
  return readerBackend().app.ping(msg);
}

/** Formats a {@link PingResponse} for display. */
export function formatPing(response: PingResponse): string {
  return `${response.echoed} (shell v${response.version})`;
}
