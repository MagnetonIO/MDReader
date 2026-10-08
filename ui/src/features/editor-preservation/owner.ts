/** Real editor admission shared by the two independently issued native requests. */
import { editorAdmission, type EditorHold } from './admission';
import { boundedJson, sameIdentity, sameRequest, submitEditor, validRequest } from './transport';
import { useEditorStore } from '../../stores/editor';
import { useProviderEdits } from '../provider-editing/store';
import { verifyPreservationIdentity, type FlushRequest } from '../../shell/persistence';

type Channel = 'editor' | 'json';
interface Joined { request: FlushRequest; channel: Channel }
interface Held {
  hold: EditorHold;
  joined: Joined[];
  drained: Promise<void>;
  generation: number | null;
  editorSubmission?: Promise<void>;
}
export function createRendererEditorOwner(resumed: () => void, paused: () => void) {
  let active: Held | null = null;
  const retired: Joined[] = [];
  function join(request: FlushRequest, channel: Channel): Held {
    if (!validRequest(request)) throw new Error('Invalid preservation request');
    verifyPreservationIdentity(request);
    if (retired.some((r) => r.channel === channel && sameRequest(r.request, request))) throw new Error('Retired preservation request');
    if (active) {
      if (!sameIdentity(active.joined[0]!.request, request)) throw new Error('Editor belongs to another renderer');
      const previous = active.joined.find((r) => r.channel === channel);
      if (previous && !sameRequest(previous.request, request)) throw new Error('Another editor preparation is held');
      if (!previous) active.joined.push({ request: structuredClone(request), channel });
      return active;
    }
    const hold = editorAdmission.hold();
    if (!hold) throw new Error('Editor preparation is already held');
    const held: Held = { hold, joined: [{ request: structuredClone(request), channel }], drained: Promise.resolve(), generation: null };
    active = held;
    paused();
    held.drained = hold.drain().then(() => {
      if (active !== held) throw new Error('Editor preparation retired');
      held.generation = hold.generation();
      if (held.generation === null) throw new Error('Editor preparation changed');
    });
    return held;
  }
  function verify(held: Held) {
    if (active !== held || held.generation === null || held.hold.generation() !== held.generation) {
      throw new Error('Editor preparation changed');
    }
    verifyPreservationIdentity(held.joined[0]!.request);
  }
  return {
    async prepareJson(request: FlushRequest) {
      const held = join(request, 'json');
      await held.drained;
      verify(held);
    },
    async prepareEditor(request: FlushRequest) {
      const held = join(request, 'editor');
      held.editorSubmission ??= (async () => {
        await held.drained;
        verify(held);
        const snapshot = held.hold.snapshot(useEditorStore.getState().entries);
        const providers = Object.values(useProviderEdits.getState().edits);
        if (providers.length > 32) throw new Error('Too many retained provider edits');
        const providerEditsJson = boundedJson(providers);
        await submitEditor({ request: structuredClone(request), ...snapshot, providerEditsJson });
        verify(held);
      })();
      return held.editorSubmission;
    },
    verifySnapshot() {
      if (!active) throw new Error('Editor is not held');
      verify(active);
    },
    release(request: FlushRequest, channel: Channel): boolean {
      if (!validRequest(request)) return false;
      if (!retired.some((r) => r.channel === channel && sameRequest(r.request, request))) {
        retired.push({ channel, request: structuredClone(request) });
        if (retired.length > 128) retired.shift();
      }
      if (!active) return false;
      const held = active;
      const index = held.joined.findIndex((r) => r.channel === channel && sameRequest(r.request, request));
      if (index < 0) return false;
      held.joined.splice(index, 1);
      if (!held.joined.length) {
        active = null;
        if (!held.hold.release()) throw new Error('Editor release was not confirmed');
        resumed();
      }
      return true;
    },
    held: () => active !== null,
  };
}
export type RendererEditorOwner = ReturnType<typeof createRendererEditorOwner>;
