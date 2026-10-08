/** AppShell supplies its actual unsaved-editor guard and latest serialization;
 * this renderer participant never discovers profiles or grants source access. */
import { listen } from '@tauri-apps/api/event';
import { flushForPreservation, releasePreservation, type FlushRequest, type LatestPersistence } from './persistence.js';

export interface ProfileTransitionBridge {
  /** Verify the held editor capability. Actual save/discard preflight occurs
   * before native global denial; ordinary save cannot run inside this hook. */
  verifyEditorFrozen: (request: FlushRequest) => Promise<void>;
  released?: (request: FlushRequest) => void;
  latest: () => LatestPersistence;
  /** Fixed UI diagnostic, never stringify private native errors. */
  failed: () => void;
  /** Re-enqueue current state after native capability release. */
  resumed: () => void;
}
interface Attempt {
  request: FlushRequest;
  controller: AbortController;
  flushing: boolean;
}
function sameRequest(left: FlushRequest, right: FlushRequest): boolean {
  return left.requestId === right.requestId
    && left.identity.ownerEpoch === right.identity.ownerEpoch
    && left.identity.rendererEpoch === right.identity.rendererEpoch;
}

export async function installProfileTransitionBridge(bridge: ProfileTransitionBridge): Promise<() => void> {
  let disposed = false;
  let active: Attempt | null = null;
  // Bound replay suppression; native remains authoritative for expired tokens.
  const retired: FlushRequest[] = [];
  function retire(request: FlushRequest) {
    retired.push({ requestId: request.requestId, identity: { ...request.identity } });
    if (retired.length > 64) retired.shift();
  }
  const offRelease = await listen<FlushRequest>('mdreader://profile-preservation-released', ({ payload }) => {
    if (!retired.some((request) => sameRequest(request, payload))) retire(payload);
    if (!active || !sameRequest(active.request, payload)) return;
    active.controller.abort();
    // A failed preflight has no persistence gate to release, but its matching
    // native release must still retire the bridge attempt and admit a fresh one.
    releasePreservation(active.request);
    bridge.released?.(active.request);
    active = null;
    if (disposed) offRelease();
    else bridge.resumed();
  });
  try {
    const offRequest = await listen<FlushRequest>('mdreader://profile-preservation-flush', ({ payload }) => {
      if (disposed || active || retired.some((request) => sameRequest(request, payload))) return;
      const attempt: Attempt = {
        request: { requestId: payload.requestId, identity: { ...payload.identity } },
        controller: new AbortController(), flushing: false,
      };
      active = attempt;
      void (async () => {
        try {
          await bridge.verifyEditorFrozen(attempt.request);
          if (disposed || active !== attempt) return;
          attempt.flushing = true;
          await flushForPreservation(attempt.request, bridge.latest, attempt.controller.signal);
        } catch {
          if (!disposed && active === attempt) bridge.failed();
        }
      })();
    });
    return () => {
      if (disposed) return;
      disposed = true;
      offRequest();
      active?.controller.abort();
      // Disposal cannot reopen a possibly admitted native freeze. Keep only its
      // exact release listener, without UI callbacks, until native settles it.
      if (!active) offRelease();
    };
  } catch (error) { offRelease(); throw error; }
}
