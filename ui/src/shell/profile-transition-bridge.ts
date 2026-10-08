/** AppShell supplies its actual unsaved-editor guard and latest serialization;
 * this renderer participant never discovers profiles or grants source access. */
import { listen } from '@tauri-apps/api/event';
import { flushForPreservation, releasePreservation, type FlushRequest, type LatestPersistence } from './persistence.js';

export interface ProfileTransitionBridge {
  /** Verify the held editor capability. Actual save/discard preflight occurs
   * before native global denial; ordinary save cannot run inside this hook. */
  verifyEditorFrozen: () => Promise<void>;
  latest: () => LatestPersistence;
  /** Fixed UI diagnostic, never stringify private native errors. */
  failed: () => void;
  /** Re-enqueue current state after native capability release. */
  resumed: () => void;
}
export async function installProfileTransitionBridge(bridge: ProfileTransitionBridge): Promise<() => void> {
  let disposed = false;
  let active: string | null = null;
  const offRelease = await listen<FlushRequest>('mdreader://profile-preservation-released', ({ payload }) => {
    if (disposed || active !== payload.requestId || !releasePreservation(payload)) return;
    active = null;
    bridge.resumed();
  });
  try {
    const offRequest = await listen<FlushRequest>('mdreader://profile-preservation-flush', ({ payload }) => {
      if (disposed || active !== null) return;
      active = payload.requestId;
      void (async () => {
        try {
          await bridge.verifyEditorFrozen();
          if (disposed) return;
          await flushForPreservation(payload, bridge.latest);
        } catch { if (!disposed) bridge.failed(); }
      })();
    });
    return () => { disposed = true; offRequest(); offRelease(); };
  } catch (error) { offRelease(); throw error; }
}
