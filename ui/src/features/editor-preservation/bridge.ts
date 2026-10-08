/** Mounted native protocol adapter. Uncertain native operations keep admission
 * held even after React disposal; only exact native releases may reopen it. */
import { listen } from '@tauri-apps/api/event';
import { createRendererEditorOwner } from './owner';
import { validRequest } from './transport';
import { installProfileTransitionBridge } from '../../shell/profile-transition-bridge';
import type { FlushRequest, LatestPersistence } from '../../shell/persistence';

export async function installRendererPreservation(callbacks: {
  latest: () => LatestPersistence;
  failed: () => void;
  paused: () => void;
  resumed: () => void;
}): Promise<() => void> {
  let disposed = false;
  let offEditorRelease: (() => void) | undefined;
  const owner = createRendererEditorOwner(() => {
    // A matching JSON release may still be unwinding its own participant.
    queueMicrotask(() => {
      if (disposed) offEditorRelease?.();
      else callbacks.resumed();
    });
  }, callbacks.paused);
  offEditorRelease = await listen<unknown>('mdreader://profile-editor-released', ({ payload }) => {
    if (validRequest(payload)) owner.release(payload, 'editor');
  });
  let offEditorRequest: (() => void) | undefined;
  let offJson: (() => void) | undefined;
  try {
    offEditorRequest = await listen<FlushRequest>('mdreader://profile-editor-freeze', ({ payload }) => {
      if (disposed || !validRequest(payload)) return;
      void owner.prepareEditor(payload).catch(() => {
        if (!disposed && owner.held()) callbacks.failed();
      });
    });
    offJson = await installProfileTransitionBridge({
      verifyEditorFrozen: (request) => owner.prepareJson(request),
      released: (request) => { owner.release(request, 'json'); },
      latest: () => { owner.verifySnapshot(); return callbacks.latest(); },
      failed: () => { if (!disposed) callbacks.failed(); },
      // The editor owner joins both releases before admitting ordinary writes.
      resumed: () => {},
    });
  } catch (error) {
    disposed = true;
    offEditorRequest?.();
    offJson?.();
    if (!owner.held()) offEditorRelease();
    throw error;
  }
  return () => {
    if (disposed) return;
    disposed = true;
    offEditorRequest?.();
    offJson?.();
    if (!owner.held()) offEditorRelease?.();
  };
}
