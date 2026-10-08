import type { ProviderReaderReference } from '../../platform/providers';
import type { CanonicalReaderBackend } from './contracts';

const MAX_BYTES = 4 * 1024 * 1024;

/** Renderer resource limits supplement native source/revision/path authority. */
export class LocalImages {
  private readonly urls = new Map<string, string>();
  private readonly leases = new Map<string, Set<string>>();
  private readonly pending = new Map<string, Promise<void>>();
  private readonly listeners = new Set<() => void>();
  private disposed = false;
  constructor(
    private readonly reference: ProviderReaderReference,
    private readonly read: NonNullable<CanonicalReaderBackend['readImage']>,
  ) {}
  subscribe = (listener: () => void) => { this.listeners.add(listener); return () => { this.listeners.delete(listener); }; };
  url = (target: string) => this.urls.get(target) ?? null;
  retain(target: string, lease: string) {
    const leases = this.leases.get(target) ?? new Set();
    leases.add(lease); this.leases.set(target, leases);
  }
  release(target: string, lease: string) {
    const leases = this.leases.get(target);
    leases?.delete(lease);
    if (!leases?.size) { this.leases.delete(target); this.retire(target); }
  }
  async load(target: string) {
    if (this.disposed || !this.leases.has(target) || this.urls.has(target)) return;
    const existing = this.pending.get(target);
    if (existing) return existing;
    if (this.pending.size >= 2) throw new Error('Two images are loading. Try this image again in a moment.');
    const operation = this.read(this.reference, target).then((value) => {
      if (this.disposed || !this.leases.has(target)) return;
      if (!value || !['image/png', 'image/jpeg'].includes(value.mime)
        || typeof value.base64 !== 'string' || !value.base64.length
        || value.base64.length > Math.ceil(MAX_BYTES / 3) * 4) {
        throw new Error('This image is outside the supported local image limits.');
      }
      const decoded = atob(value.base64);
      if (!decoded.length || decoded.length > MAX_BYTES) throw new Error('This image exceeds the local image limit.');
      const bytes = Uint8Array.from(decoded, (character) => character.charCodeAt(0));
      // Eviction never triggers automatic reload: the visible placeholder offers
      // an explicit load action, keeping at most two decoded copies alive.
      if (this.urls.size >= 2) this.retire(this.urls.keys().next().value!);
      this.urls.set(target, URL.createObjectURL(new Blob([bytes], { type: value.mime })));
      this.changed();
    }).finally(() => { this.pending.delete(target); });
    this.pending.set(target, operation);
    return operation;
  }
  dispose() {
    this.disposed = true;
    this.leases.clear();
    for (const target of [...this.urls.keys()]) this.retire(target);
    this.listeners.clear();
  }
  private retire(target: string) {
    const url = this.urls.get(target);
    if (url) { URL.revokeObjectURL(url); this.urls.delete(target); this.changed(); }
  }
  private changed() { for (const listener of this.listeners) listener(); }
}

/** Do not normalize or reconstruct native paths in the renderer. */
export function relativeImageTarget(value: string | undefined): string | null {
  if (!value || value.length > 4096 || /[\\:#?]/.test(value)
    || [...value].some((character) => character.charCodeAt(0) < 32 || character.charCodeAt(0) === 127)
    || value.startsWith('/')) return null;
  return value;
}
