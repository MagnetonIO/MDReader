import { commands, type PingResponse } from '../bindings';

/**
 * Thin wrapper over the generated `ping` command. This is the single place the
 * webview crosses the Tauri IPC boundary in the P0 scaffold; it exists to prove
 * the JS -> Rust -> JS round-trip (and the tauri-specta type flow) works.
 */
export async function pingBackend(msg: string): Promise<PingResponse> {
  return commands.ping(msg);
}

/** Formats a {@link PingResponse} for display. */
export function formatPing(response: PingResponse): string {
  return `${response.echoed} (shell v${response.version})`;
}
