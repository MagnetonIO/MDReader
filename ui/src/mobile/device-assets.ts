import type { AssetManifest, AssetReceipt, AssetReply, AssetRequest } from '../bindings';
import { readerBackend } from '../platform/backend';
export type AssetBackend = (request: AssetRequest) => Promise<AssetReply>;
export const nativeAssetBackend: AssetBackend = async (request) => {
  const result = await readerBackend().mobile.mobileReaderAssets(request);
  if (result.status === 'error') throw new Error(result.error);
  return result.data;
};
const CHUNK = 64 * 1024;
export const MAX_ASSET_BYTES = 4 * 1024 * 1024;
export function assetTarget(raw: string): string | null {
  if (new TextEncoder().encode(raw).length > 1536) return null;
  let value: string;
  try { value = decodeURIComponent(raw); } catch { return null; }
  if (!value || new TextEncoder().encode(value).length > 512 || value.startsWith('/') || /[:\\?#%]/u.test(value)
    || Array.from(value).some((character) => { const code = character.codePointAt(0)!; return code < 32 || code >= 127 && code <= 159; })) return null;
  const parts = value.split('/').filter((part) => part !== '.');
  if (!parts.length || parts.length > 8 || parts.some((part) => !part || part === '..')) return null;
  return parts.join('/');
}
function encode(bytes: Uint8Array): string {
  let binary = '';
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}
function decode(encoded: string): Uint8Array<ArrayBuffer> {
  if (encoded.length > Math.ceil(CHUNK / 3) * 4) throw new Error('Asset chunk exceeds its limit.');
  return Uint8Array.from(atob(encoded), (character) => character.charCodeAt(0));
}
export async function uploadAsset(backend: AssetBackend, manifest: AssetManifest, target: string, file: File, pendingId?: string, signal?: AbortSignal): Promise<void> {
  const check = () => { if (signal?.aborted) throw new Error('The document selection changed. The pending copy remains available for explicit recovery.'); };
  check();
  if (!file.size || file.size > MAX_ASSET_BYTES || !manifest.targets.includes(target)) throw new Error('Choose a selected image of at most 4 MiB.');
  let intentId = pendingId;
  if (intentId) {
    const pending = manifest.pending.find((item) => item.intent_id === intentId && item.target === target && item.bytes === file.size && item.state === 'reserved');
    if (!pending) throw new Error('The pending selection or file size changed.');
  } else {
    const begin = await backend({ operation: 'begin', document_id: manifest.document_id, revision: manifest.document_revision, target, bytes: file.size, expected_manifest: manifest.manifest });
    check();
    if (begin.operation !== 'intent') throw new Error('Asset upload was not reserved.');
    intentId = begin.intent_id;
  }
  // Replay from zero after interruption. The native owner compares previously
  // written chunks exactly; it never trusts a newly chosen file's name.
  for (let offset = 0; offset < file.size; offset += CHUNK) {
    check();
    const bytes = new Uint8Array(await file.slice(offset, offset + CHUNK).arrayBuffer());
    check();
    const reply = await backend({ operation: 'write', intent_id: intentId, offset, base64: encode(bytes) });
    if (reply.operation !== 'intent' || reply.intent_id !== intentId || reply.received < offset + bytes.length || reply.received > file.size) throw new Error('Asset upload acknowledgement is invalid.');
  }
  check();
  const published = await backend({ operation: 'commit', intent_id: intentId });
  if (published.operation !== 'published' || published.receipt.asset_id !== intentId
    || published.receipt.document_id !== manifest.document_id || published.receipt.document_revision !== manifest.document_revision || published.receipt.target !== target) throw new Error('Asset publication could not be verified. Reload the manifest to reconcile.');
}

type CachedImage = { url: string; sequence: number };
/** Two in-flight reads and two decoded image URLs per selected revision. */
export class DeviceAssetImages {
  private epoch = 0;
  private running = 0;
  private queue: Array<{ receipt: AssetReceipt; resolve: () => void; reject: (error: Error) => void; epoch: number }> = [];
  private inflight = new Map<string, Promise<void>>();
  private images = new Map<string, CachedImage>();
  private listeners = new Set<() => void>();
  private sessions = new Set<string>();
  private sequence = 0;
  private leases = new Map<string, Set<string>>();
  private disposed = false;
  constructor(readonly manifest: AssetManifest, private backend: AssetBackend = nativeAssetBackend) {}
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  url = (assetId: string): string | null => this.images.get(assetId)?.url ?? null;
  private changed() { for (const listener of this.listeners) listener(); }
  load(receipt: AssetReceipt): Promise<void> {
    if (this.disposed || !this.leases.get(receipt.asset_id)?.size) return Promise.reject(new Error('The document selection changed.'));
    if (this.images.has(receipt.asset_id)) return Promise.resolve();
    const old = this.inflight.get(receipt.asset_id); if (old) return old;
    const selected = this.manifest.assets.find((item) => item.asset_id === receipt.asset_id);
    if (!selected || this.queue.length >= 8) return Promise.reject(new Error('This image is not selected.'));
    const promise = new Promise<void>((resolve, reject) => { this.queue.push({ receipt: selected, resolve, reject, epoch: this.epoch }); });
    this.inflight.set(receipt.asset_id, promise);
    this.drain(); return promise;
  }
  retain(assetId: string, lease: string) {
    const leases = this.leases.get(assetId) ?? new Set<string>(); leases.add(lease); this.leases.set(assetId, leases);
  }
  release(assetId: string, lease: string) {
    const leases = this.leases.get(assetId); leases?.delete(lease);
    if (leases?.size) return; this.leases.delete(assetId); this.evict(assetId);
    const removed = this.queue.filter((item) => item.receipt.asset_id === assetId);
    this.queue = this.queue.filter((item) => item.receipt.asset_id !== assetId);
    for (const item of removed) item.reject(new Error('The image is no longer visible.'));
    if (removed.length) this.inflight.delete(assetId);
  }
  private evict(assetId: string) {
    const cached = this.images.get(assetId);
    if (cached) { URL.revokeObjectURL(cached.url); this.images.delete(assetId); this.changed(); }
  }
  /** Explicit transfer yields this owner's image permits before a native copy
   * acquires one. Late opens settle and close rather than leaking a session. */
  async disposeAndDrain(): Promise<void> {
    const pending = [...this.inflight.values()];
    const sessions = [...this.sessions];
    this.dispose();
    await Promise.allSettled([
      ...pending,
      ...sessions.map((session_id) => this.backend({ operation: 'close', session_id })),
    ]);
  }
  dispose() {
    this.disposed = true; this.epoch++;
    for (const pending of this.queue) pending.reject(new Error('The document selection changed.'));
    this.queue = [];
    for (const cached of this.images.values()) URL.revokeObjectURL(cached.url);
    this.images.clear(); this.changed();
    for (const sessionId of this.sessions) void this.backend({ operation: 'close', session_id: sessionId }).catch(() => {});
  }
  private drain() {
    while (!this.disposed && this.running < 2 && this.queue.length) {
      const item = this.queue.shift()!; this.running++;
      void this.read(item.receipt, item.epoch).then(item.resolve, item.reject).finally(() => {
        this.running--; this.inflight.delete(item.receipt.asset_id); this.drain();
      });
    }
  }
  private async read(receipt: AssetReceipt, epoch: number) {
    if (!Number.isInteger(receipt.bytes) || receipt.bytes < 1 || receipt.bytes > MAX_ASSET_BYTES
      || !Number.isInteger(receipt.width) || !Number.isInteger(receipt.height)
      || receipt.width < 1 || receipt.height < 1 || receipt.width > 4096 || receipt.height > 4096 || receipt.width * receipt.height > 4_000_000 || !['image/png', 'image/jpeg'].includes(receipt.mime)
      || receipt.document_id !== this.manifest.document_id || receipt.document_revision !== this.manifest.document_revision) throw new Error('The image receipt is invalid.');
    const opened = await this.backend({ operation: 'open', document_id: receipt.document_id, revision: receipt.document_revision, asset_id: receipt.asset_id, manifest: this.manifest.manifest });
    if (opened.operation !== 'opened') throw new Error('The selected image is unavailable.');
    const sessionId = opened.session_id; this.sessions.add(sessionId);
    try {
      if (opened.receipt.asset_id !== receipt.asset_id || opened.receipt.document_id !== receipt.document_id
        || opened.receipt.document_revision !== receipt.document_revision || opened.receipt.content_hash !== receipt.content_hash
        || opened.receipt.target !== receipt.target || opened.receipt.bytes !== receipt.bytes || opened.receipt.mime !== receipt.mime
        || opened.receipt.width !== receipt.width || opened.receipt.height !== receipt.height) throw new Error('The image receipt changed.');
      const chunks: Uint8Array<ArrayBuffer>[] = [];
      for (let offset = 0; offset < receipt.bytes; offset += CHUNK) {
        if (epoch !== this.epoch || !this.leases.get(receipt.asset_id)?.size) throw new Error('The image selection or visibility changed.');
        const reply = await this.backend({ operation: 'chunk', session_id: sessionId, offset });
        if (reply.operation !== 'chunk' || reply.offset !== offset) throw new Error('Invalid image chunk response.');
        const bytes = decode(reply.base64); const expected = Math.min(CHUNK, receipt.bytes - offset);
        if (bytes.length !== expected || reply.complete !== (offset + expected === receipt.bytes)) throw new Error('The image copy is incomplete.');
        chunks.push(bytes);
      }
      if (epoch !== this.epoch || !this.leases.get(receipt.asset_id)?.size) throw new Error('The image selection or visibility changed.');
      while (this.images.size >= 2) {
        const oldest = [...this.images].sort((left, right) => left[1].sequence - right[1].sequence)[0];
        if (oldest) this.evict(oldest[0]); else break;
      }
      this.images.set(receipt.asset_id, { url: URL.createObjectURL(new Blob(chunks, { type: receipt.mime })), sequence: ++this.sequence }); this.changed();
    } finally {
      this.sessions.delete(sessionId);
      await this.backend({ operation: 'close', session_id: sessionId }).catch(() => {});
    }
  }
}
