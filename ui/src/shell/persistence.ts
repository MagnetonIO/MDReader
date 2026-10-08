/** Strict native JSON persistence. Plugin stores never open these two files. */
import { invoke } from '@tauri-apps/api/core';
import { deserializeSession, sanitizeSettings, type RestoredSession } from '../stores/session.js';
import { restoreReaderHistory } from '../features/reader-history/serialization';
import type { RestoredHistory } from '../features/reader-history/model';
import { createSessionPatchWriter, type SessionPatch } from '../features/reader-history/session-patches';
import type { SettingsSnapshot } from '../stores/settings.js';

export function isTauri(): boolean {
  return typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window;
}

export interface SerialWriter<T> {
  write: (value: T) => void;
  /** Reject if the newest requested value failed; a later success supersedes it. */
  drain: () => Promise<void>;
}

/** Single flight/newest wins, with a strict latest-generation drain. */
export function createSerialWriter<T>(write: (value: T) => Promise<void>): SerialWriter<T> {
  let inFlight: Promise<void> | null = null;
  let pending: { value: T; generation: number } | null = null;
  let requested = 0;
  let failure: { generation: number; error: unknown } | null = null;
  function pump(value: T, generation: number): void {
    let operation: Promise<void>;
    try { operation = write(value); } catch (error) { operation = Promise.reject(error); }
    inFlight = operation.then(
      () => { if (failure && failure.generation <= generation) failure = null; },
      (error: unknown) => { failure = { generation, error }; },
    ).finally(() => {
      inFlight = null;
      if (pending) { const next = pending; pending = null; pump(next.value, next.generation); }
    });
  }
  return {
    write(value) {
      const generation = ++requested;
      if (inFlight) pending = { value, generation };
      else pump(value, generation);
    },
    async drain() {
      while (inFlight) await inFlight;
      if (failure?.generation === requested) throw failure.error;
    },
  };
}

export interface RendererIdentity { ownerEpoch: string; rendererEpoch: string }
export interface WriteReceipt extends RendererIdentity {
  kind: 'session' | 'settings'; generation: number; sha256: string;
}
export interface FlushRequest { requestId: string; identity: RendererIdentity }
interface NativeLoaded {
  identity: RendererIdentity; sessionJson: string | null; settingsJson: string | null;
}
export interface LatestPersistence { session: SessionPatch; settings: SettingsSnapshot }
export interface LoadedState {
  session: RestoredSession | null; settings: Partial<SettingsSnapshot>;
  /** Fixed diagnostic only; defaults after this failure must not be saved. */
  restoreFailed?: true;
  readerHistory?: RestoredHistory;
}

let registration: Promise<NativeLoaded> | undefined;
let host: Promise<'native_atomic' | 'legacy_host'> | undefined;
let sessionGeneration = 0;
let settingsGeneration = 0;
let preservation: FlushRequest | null = null;
let hydrationAttempt = 0;
let hydrated: { attempt: number; identity: RendererIdentity } | null = null;

function sameIdentity(left: RendererIdentity, right: RendererIdentity): boolean {
  return left.ownerEpoch === right.ownerEpoch && left.rendererEpoch === right.rendererEpoch;
}
function requireHydration(identity: RendererIdentity, attempt = hydrationAttempt): void {
  if (!hydrated || hydrated.attempt !== attempt || attempt !== hydrationAttempt
    || !sameIdentity(hydrated.identity, identity)) throw new Error('Native persistence is not hydrated');
}

function registered(): Promise<NativeLoaded> {
  if (!registration) {
    const pending = invoke<NativeLoaded>('native_profile_persistence_load').catch((error: unknown) => {
      // A lost first reply must not poison every later explicit reload. The
      // next registration rotates the native renderer epoch; hydration still
      // has to read and verify that exact new identity before permitting saves.
      if (registration === pending) registration = undefined;
      throw error;
    });
    registration = pending;
  }
  return registration;
}
function persistenceHost(): Promise<'native_atomic' | 'legacy_host'> {
  if (!host) {
    const pending = invoke<'native_atomic' | 'legacy_host'>('native_profile_persistence_host').then((mode) => {
      if (mode !== 'native_atomic' && mode !== 'legacy_host') throw new Error('Invalid persistence host');
      return mode;
    }).catch((error: unknown) => {
      if (host === pending) host = undefined;
      throw error;
    });
    host = pending;
  }
  return host;
}
function next(kind: 'session' | 'settings'): number {
  const generation = kind === 'session' ? ++sessionGeneration : ++settingsGeneration;
  if (generation > 0x7fff_ffff) throw new Error('Persistence generation exhausted');
  return generation;
}
function verifyReceipt(receipt: WriteReceipt, identity: RendererIdentity, kind: 'session' | 'settings', generation: number): void {
  if (receipt.kind !== kind || receipt.generation !== generation || receipt.ownerEpoch !== identity.ownerEpoch
    || receipt.rendererEpoch !== identity.rendererEpoch || !/^[a-f0-9]{64}$/.test(receipt.sha256)) {
    throw new Error('Invalid persistence receipt');
  }
}

/** Defaults after a failed native read are render-only. Only the newest explicit
 * successful load admits writes; older concurrent loads cannot reopen admission. */
export async function loadPersisted(): Promise<LoadedState> {
  if (!isTauri()) return { session: null, settings: {} };
  const attempt = ++hydrationAttempt;
  hydrated = null;
  try {
    if (await persistenceHost() === 'legacy_host') {
      const legacy = await import('./legacy-persistence.js');
      const state = await legacy.readLegacyPersistence();
      return { session: deserializeSession(state.session), settings: sanitizeSettings(state.settings), readerHistory: restoreReaderHistory(undefined, false) };
    }
    const registration = await registered();
    const loaded = await invoke<NativeLoaded>('native_profile_persistence_read', { identity: registration.identity });
    if (!sameIdentity(loaded.identity, registration.identity)) throw new Error('Invalid persistence identity');
    const session: unknown = loaded.sessionJson === null ? null : JSON.parse(loaded.sessionJson);
    const settings: unknown = loaded.settingsJson === null ? null : JSON.parse(loaded.settingsJson);
    const rawSession = isObject(session) ? session.session : undefined;
    const reader = rawSession !== undefined && (!isObject(rawSession) || rawSession.version !== 1)
      ? restoreReaderHistory({ version: -1 }, true)
      : restoreReaderHistory(isObject(rawSession) ? rawSession.readerHistory : undefined, true);
    const state = {
      readerHistory: reader,
      session: deserializeSession(rawSession),
      settings: sanitizeSettings(isObject(settings) ? settings.settings : null),
    };
    if (attempt === hydrationAttempt) hydrated = { attempt, identity: { ...loaded.identity } };
    return state;
  } catch { return { session: null, settings: {}, restoreFailed: true }; }
}
function isObject(value: unknown): value is Record<string, unknown> {
  return typeof value === 'object' && value !== null && !Array.isArray(value);
}
async function write(kind: 'session' | 'settings', data: SessionPatch | SettingsSnapshot): Promise<void> {
  if (!isTauri()) return;
  const admission = hydrated;
  if (preservation) throw new Error('Persistence is frozen for preservation');
  if (await persistenceHost() === 'legacy_host') {
    const legacy = await import('./legacy-persistence.js');
    return legacy.writeLegacyPersistence(kind, data);
  }
  if (!admission) throw new Error('Native persistence is not hydrated');
  const loaded = await registered();
  requireHydration(loaded.identity, admission.attempt);
  if (preservation) throw new Error('Persistence is frozen for preservation');
  const generation = next(kind);
  const receipt = await invoke<WriteReceipt>('native_profile_persistence_write', {
    identity: loaded.identity, kind, generation, json: JSON.stringify(data),
  });
  verifyReceipt(receipt, loaded.identity, kind, generation);
}
export async function saveSession(data: SessionPatch): Promise<void> { await write('session', data); }
export const sessionWriter = createSessionPatchWriter(saveSession, createSerialWriter);
const settingsWriter = createSerialWriter<SettingsSnapshot>((data) => write('settings', data));
export async function saveSettings(snapshot: SettingsSnapshot): Promise<void> {
  settingsWriter.write(snapshot);
  await settingsWriter.drain();
}

/** The separate bridge calls this after the real unsaved-editor guard. Only a
 * fresh native request token can route the latest snapshot through freeze. */
export async function flushForPreservation(
  request: FlushRequest, latest: () => LatestPersistence, signal?: AbortSignal,
): Promise<void> {
  // Retain our identity before yielding, not a caller-owned mutable payload.
  const held = { requestId: request.requestId, identity: { ...request.identity } };
  const requireActive = () => {
    if (signal?.aborted) throw new Error('Preservation request retired');
  };
  requireActive();
  const admission = hydrated;
  if (await persistenceHost() !== 'native_atomic') throw new Error('Profile preservation is unavailable on this host');
  requireActive();
  if (!admission) throw new Error('Native persistence is not hydrated');
  const loaded = await registered();
  requireActive();
  const attempt = admission.attempt;
  requireHydration(loaded.identity, attempt);
  if (held.identity.ownerEpoch !== loaded.identity.ownerEpoch || held.identity.rendererEpoch !== loaded.identity.rendererEpoch
    || preservation) throw new Error('Stale preservation request');
  preservation = held;
  // Ordinary queued writes may be refused by native freeze. Await settlement;
  // the scoped latest write then supersedes those earlier generations.
  await Promise.allSettled([sessionWriter.drain(), settingsWriter.drain()]);
  requireActive();
  if (preservation !== held) throw new Error('Preservation request retired');
  requireHydration(loaded.identity, attempt);
  const snapshot = latest();
  requireActive();
  if (preservation !== held) throw new Error('Preservation request retired');
  const sessionGeneration = next('session');
  const settingsGeneration = next('settings');
  const receipts = await invoke<[WriteReceipt, WriteReceipt]>('native_profile_persistence_flush', {
    submission: { request: held, sessionGeneration, settingsGeneration,
      sessionJson: JSON.stringify(snapshot.session), settingsJson: JSON.stringify(snapshot.settings) },
  });
  requireActive();
  if (preservation !== held) throw new Error('Preservation request retired');
  verifyReceipt(receipts[0], loaded.identity, 'session', sessionGeneration);
  verifyReceipt(receipts[1], loaded.identity, 'settings', settingsGeneration);
}
/** Stay denied until final native verification/snapshot releases the request. */
export function releasePreservation(request: FlushRequest): boolean {
  if (!preservation || preservation.requestId !== request.requestId
    || preservation.identity.ownerEpoch !== request.identity.ownerEpoch
    || preservation.identity.rendererEpoch !== request.identity.rendererEpoch) return false;
  preservation = null;
  return true;
}
