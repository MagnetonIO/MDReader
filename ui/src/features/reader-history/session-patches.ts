/** One session queue coalesces unacknowledged field groups, never independent writers. */
import type { SessionData } from '../../stores/session';
import type { SerialWriter } from '../../shell/persistence';
import type { ReaderHistory } from './model';
export type SessionPatch = { version: number; readerHistory?: ReaderHistory } & Partial<Omit<SessionData, 'version'>>;
const encode = (value: unknown) => JSON.stringify(value);
export function createSessionPatchWriter(
  save: (patch: SessionPatch) => Promise<void>,
  serial: <T>(save: (value: T) => Promise<void>) => SerialWriter<T>,
): SerialWriter<SessionPatch> {
  let outstanding: SessionPatch = { version: 1 };
  const writer = serial<SessionPatch>(async (snapshot) => {
    await save(snapshot);
    const remaining: Record<string, unknown> = { ...outstanding };
    for (const [key, value] of Object.entries(snapshot)) {
      if (key !== 'version' && encode(remaining[key]) === encode(value)) delete remaining[key];
    }
    outstanding = remaining as SessionPatch;
  });
  return {
    write(patch) {
      outstanding = { ...outstanding, ...patch };
      writer.write({ ...outstanding });
    },
    drain: writer.drain,
  };
}
export function createSessionTracker(legacy: SessionData, reader: ReaderHistory) {
  let previousLegacy = legacy;
  let previousReader = encode(reader);
  return {
    next(legacy: SessionData, reader: ReaderHistory | null): SessionPatch | null {
      const patch: SessionPatch = { version: 1 };
      for (const key of ['root', 'activeTabId', 'tabs', 'recent'] as const) {
        if (encode(legacy[key]) !== encode(previousLegacy[key])) Object.assign(patch, { [key]: legacy[key] });
      }
      previousLegacy = legacy;
      if (reader && encode(reader) !== previousReader) {
        patch.readerHistory = reader;
        previousReader = encode(reader);
      }
      return Object.keys(patch).length === 1 ? null : patch;
    },
  };
}
