import { readerBackend, unwrapNative } from './backend';

/** Native WebViews delegate to the OS opener. No fallback WebView navigation
 * after a failed native open; the caller keeps the reader and reports failure. */
export async function openReaderExternal(url: string): Promise<void> {
  let parsed: URL;
  try { parsed = new URL(url); } catch { throw new Error('This external link is invalid.'); }
  if (new TextEncoder().encode(url).length > 4096 || /[\u0000-\u001f\u007f]/.test(url)
    || !['https:', 'http:', 'mailto:', 'tel:'].includes(parsed.protocol)
    || parsed.username || parsed.password) throw new Error('This external link is invalid.');
  try {
    if ('__TAURI_INTERNALS__' in window) await unwrapNative(readerBackend().externalLinks.readerOpenExternal(url));
    else window.open(url, '_blank', 'noopener,noreferrer');
  } catch {
    throw new Error('The link could not be opened. Your document is still available here.');
  }
}
