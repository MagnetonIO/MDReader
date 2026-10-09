import type { CatalogPage } from './backend';
import type { ProviderRefresh } from '../../platform/providers';

const MAX_BATCHES = 64;
const MAX_WINDOW_MS = 5 * 60_000;
export interface MetadataProgress { sourceId: string; batches: number; pages: number; stopping: boolean }
export type MetadataLoadResult = 'complete' | 'paused' | 'retired';

/** One explicit user action owns a bounded, sequential metadata-only run.
 * Native refresh has no cancellation endpoint: stop waits for its current call,
 * then issues no further provider batch. Its published metadata is still shown. Never fetch document bodies. */
export async function loadMetadata({ refresh, page, current, stopped, progress, observed, publish, retire }: {
  refresh(): Promise<ProviderRefresh>;
  page(): Promise<CatalogPage>;
  current(): boolean;
  stopped(): boolean;
  progress(batches: number, pages: number): void;
  observed(complete: boolean): void;
  publish(page: CatalogPage, complete: boolean): void;
  retire(page: CatalogPage): void;
}): Promise<MetadataLoadResult> {
  const deadline = Date.now() + MAX_WINDOW_MS;
  let pages = 0;
  for (let batches = 0; batches < MAX_BATCHES; batches++) {
    if (!current()) return 'retired';
    if (stopped() || Date.now() >= deadline) return 'paused';
    progress(batches, pages);
    const result = await refresh();
    if (!current()) return 'retired';
    if (result.operation !== 'refresh' || typeof result.metadataComplete !== 'boolean' || !Number.isSafeInteger(result.pages) || result.pages < 0 || result.pages > 8) throw new Error('The metadata loading result could not be verified.');
    observed(result.metadataComplete);
    pages += result.pages;
    const value = await page();
    if (!current()) { retire(value); return 'retired'; }
    publish(value, result.metadataComplete);
    progress(batches + 1, pages);
    if (result.metadataComplete) return 'complete';
    if (stopped()) return 'paused';
  }
  return 'paused';
}
