import type { ProviderReaderReference } from '../../platform/providers';
import type { LocalImageBytes } from './contracts';

const MAX_BYTES = 4 * 1024 * 1024;
const MAX_VISIBLE_BYTES = 16 * 1024 * 1024;
const MAX_VISIBLE_PIXELS = 16_000_000;
const MAX_IMAGES = 64;
type Work = {
  target: string;
  promise: Promise<void>;
  resolve(): void;
  reject(cause: unknown): void;
  cancelled: boolean;
};
type Copy = { url: string; bytes: number; pixels: number };

/** FIFO reads at two, with explicit aggregate bounds and no visible eviction. */
export class LocalImages<Reference = ProviderReaderReference> {
  private readonly urls = new Map<string, Copy>();
  private readonly leases = new Map<string, Set<string>>();
  private readonly pending = new Map<string, Work>();
  private queue: Work[] = [];
  private active = 0;
  private encodedBytes = 0;
  private pixels = 0;
  private readonly listeners = new Set<() => void>();
  private disposed = false;
  constructor(
    private readonly reference: Reference,
    private readonly read: (reference: Reference, target: string) => Promise<LocalImageBytes>,
  ) {}
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  url = (target: string) => this.urls.get(target)?.url ?? null;
  retain(target: string, lease: string) {
    if (this.disposed) return;
    const leases = this.leases.get(target) ?? new Set();
    leases.add(lease); this.leases.set(target, leases);
  }
  release(target: string, lease: string) {
    const leases = this.leases.get(target);
    leases?.delete(lease);
    if (!leases?.size) {
      this.leases.delete(target);
      this.retire(target);
      this.cancel(target);
    }
  }
  async load(target: string) {
    if (this.disposed || !this.leases.has(target) || this.urls.has(target)) return;
    const existing = this.pending.get(target);
    if (existing) return existing.promise;
    if (this.queue.length >= MAX_IMAGES) throw new Error('Too many images are waiting to load. Scroll to this image again.');
    let resolve!: () => void;
    let reject!: (cause: unknown) => void;
    const promise = new Promise<void>((done, failed) => { resolve = done; reject = failed; });
    const work: Work = { target, promise, resolve, reject, cancelled: false };
    this.pending.set(target, work);
    this.queue.push(work);
    this.drain();
    return promise;
  }
  dispose() {
    this.disposed = true;
    this.leases.clear();
    for (const target of [...this.pending.keys()]) this.cancel(target);
    for (const target of [...this.urls.keys()]) this.retire(target);
    this.listeners.clear();
  }
  private drain() {
    while (!this.disposed && this.active < 2 && this.queue.length) {
      const work = this.queue.shift()!;
      if (work.cancelled) continue;
      this.active++;
      void this.run(work);
    }
  }
  private async run(work: Work) {
    try {
      const value = await this.read(this.reference, work.target);
      if (this.disposed || work.cancelled || !this.leases.has(work.target)) return;
      if (!value || (value.mime !== 'image/png' && value.mime !== 'image/jpeg')
        || typeof value.base64 !== 'string' || !value.base64.length
        || value.base64.length > Math.ceil(MAX_BYTES / 3) * 4
        || !Number.isInteger(value.width) || !Number.isInteger(value.height)
        || value.width < 1 || value.height < 1 || value.width > 4096 || value.height > 4096
        || value.width * value.height > 4_000_000) {
        throw new Error('This image is outside the supported local image limits.');
      }
      const decoded = atob(value.base64);
      if (!decoded.length || decoded.length > MAX_BYTES) throw new Error('This image exceeds the local image limit.');
      const pixels = value.width * value.height;
      if (this.urls.size >= MAX_IMAGES || this.encodedBytes + decoded.length > MAX_VISIBLE_BYTES
        || this.pixels + pixels > MAX_VISIBLE_PIXELS) {
        throw new Error('Too many images are visible at once. Scroll to release other images, then load this one again.');
      }
      const bytes = Uint8Array.from(decoded, (character) => character.charCodeAt(0));
      this.urls.set(work.target, { url: URL.createObjectURL(new Blob([bytes], { type: value.mime })), bytes: decoded.length, pixels });
      this.encodedBytes += decoded.length; this.pixels += pixels;
      this.changed();
    } catch (cause) {
      work.reject(cause);
    } finally {
      work.resolve();
      if (this.pending.get(work.target) === work) this.pending.delete(work.target);
      this.active--;
      this.drain();
    }
  }
  private cancel(target: string) {
    const work = this.pending.get(target);
    if (!work) return;
    work.cancelled = true; work.resolve();
    this.pending.delete(target);
    this.queue = this.queue.filter((queued) => queued !== work);
  }
  private retire(target: string) {
    const copy = this.urls.get(target);
    if (copy) {
      URL.revokeObjectURL(copy.url); this.urls.delete(target);
      this.encodedBytes -= copy.bytes; this.pixels -= copy.pixels;
      this.changed();
    }
  }
  private changed() { for (const listener of this.listeners) listener(); }
}

/** Image presentation needs leases and URLs, never the native receipt itself. */
export type LocalImageController = Pick<LocalImages, 'subscribe' | 'url' | 'retain' | 'release' | 'load'>;

/** Do not normalize or reconstruct native paths in the renderer. */
export function relativeImageTarget(value: string | undefined): string | null {
  if (!value || value.length > 4096 || /[\\:#?]/.test(value)
    || [...value].some((character) => character.charCodeAt(0) < 32 || character.charCodeAt(0) === 127)
    || value.startsWith('/')) return null;
  return value;
}
