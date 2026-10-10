import type { CatalogPage } from './backend';
import type { ProviderRefresh } from '../../platform/providers';

export interface MetadataProgress { sourceId: string; batches: number; pages: number; stopping: boolean }
export type MetadataLoadResult = 'complete' | 'paused' | 'retired';

/** One explicit user action owns a sequential metadata-only run to completion.
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
  let pages = 0;
  let noProgress = 0;
  for (let batches = 0; ; batches++) {
    if (!current()) return 'retired';
    if (stopped()) return 'paused';
    progress(batches, pages);
    const result = await refresh();
    if (result.operation !== 'refresh' || typeof result.metadataComplete !== 'boolean' || !Number.isSafeInteger(result.pages) || result.pages < 0 || result.pages > 8) throw new Error('The metadata loading result could not be verified.');
    // Native completion belongs to this run even if a file click retired UI publication.
    observed(result.metadataComplete);
    if (!current()) return 'retired';
    noProgress = result.pages === 0 && !result.metadataComplete ? noProgress + 1 : 0;
    if (noProgress >= 3) throw new Error('File discovery stopped making progress. Refresh this source to try again.');
    pages += result.pages;
    const value = await page();
    if (!current()) { retire(value); return 'retired'; }
    publish(value, result.metadataComplete);
    progress(batches + 1, pages);
    if (result.metadataComplete) return 'complete';
    if (stopped()) return 'paused';
    // Yield between bounded native batches so a click/edit can reach transport admission.
    await new Promise<void>(resolve => setTimeout(resolve, 0));
  }
}
