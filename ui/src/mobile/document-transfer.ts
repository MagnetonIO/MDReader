/** Native transport accepts an exact existing device receipt only. No Markdown,
 * filesystem paths, resource URLs or destination grants cross this boundary. */
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { CanonicalReaderReference } from '../features/reader/contracts';
import { parseReadingLink } from '../features/document-transfer/reading-link';
export type TransferCapabilities = { export: boolean; share: boolean; readingLinks: boolean };
export async function transferCapabilities(): Promise<TransferCapabilities> {
  const value = await invoke<TransferCapabilities>('mobile_reader_transfer_capabilities');
  return { export: value.export === true, share: value.share === true, readingLinks: value.readingLinks === true };
}
export async function transferDocument(reference: CanonicalReaderReference, action: 'export' | 'share'): Promise<'exported' | 'presented' | 'cancelled'> {
  if (!reference.documentId || reference.path !== 'document.md') throw new Error('Select an imported document first.');
  try {
    const value = await invoke<{ state: string }>('mobile_reader_transfer', { reference: {
      documentId: reference.documentId, sourceId: reference.sourceId, path: reference.path, revision: reference.revision,
    }, action });
    if (value.state === 'cancelled' || action === 'export' && value.state === 'exported' || action === 'share' && value.state === 'presented') return value.state as 'exported' | 'presented' | 'cancelled';
    throw new Error();
  } catch (cause) {
    if (cause === 'busy') throw new Error('Another copy is still in use. Close its sheet or try again after temporary copies expire.');
    if (cause === 'unsupported') throw new Error('This build does not have a native export or sharing adapter.');
    if (cause === 'stale') throw new Error('The document or selected images changed. Reopen it before exporting.');
    if (cause === 'too_large') throw new Error('This copy exceeds the mobile export limit. Your original is unchanged.');
    throw new Error('The copy could not be confirmed. Your original is unchanged; check your selected destination before retrying.');
  }
}
/** Subscribe before draining cold-start links. OS delivery supplies no grant:
 * parser and the caller's exact source/revision read are still authoritative. */
export async function observeReadingLinks(open: (reference: CanonicalReaderReference) => Promise<void>, failed: (message: string) => void): Promise<() => void> {
  let active = true;
  let running = false;
  const queue: string[] = [];
  const drain = async () => {
    if (running) return;
    running = true;
    try {
      while (active && queue.length) {
        const url = queue.shift()!;
        try { await open(parseReadingLink(url)); }
        catch { if (active) failed('This exact reading link is unavailable in your current library. No source was added.'); }
      }
    } finally { running = false; }
  };
  const seen = new Map<string, number>();
  const receive = (url: string) => {
    if (!active || typeof url !== 'string') return;
    const now = Date.now();
    if (now - (seen.get(url) ?? 0) < 1000) return;
    seen.set(url, now); if (seen.size > 64) seen.delete(seen.keys().next().value!);
    if (queue.length >= 8) { queue.shift(); failed('Some older reading links were skipped. Open the desired link again.'); }
    queue.push(url); void drain();
  };
  const unlisten = await listen<string>('mdreader://reading-link-open', (event) => receive(event.payload));
  try {
    const pending = await invoke<{ links: string[]; truncated: boolean }>('mobile_reader_take_reading_links');
    if (!Array.isArray(pending.links) || pending.links.length > 8) throw new Error();
    pending.links.forEach(receive);
    if (pending.truncated && active) failed('Some older reading links were skipped. Open the desired link again.');
  } catch { /* Paste remains available when OS delivery is absent. */ }
  return () => { active = false; queue.length = 0; unlisten(); };
}
