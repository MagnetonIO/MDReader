import { platform } from '@tauri-apps/plugin-os';
import { isTauri } from '../../shell/persistence';
import { readerBackend, unwrapNative } from '../../platform/backend';

/** Explicit editor snapshot, never a source receipt or destination grant. Native
 * dialogs authorize new-file creation; browsers initiate their ordinary download. */
export async function saveDraftCopy(markdown: string, filename: string): Promise<'exported' | 'cancelled' | 'downloaded'> {
  if (new TextEncoder().encode(markdown).byteLength > 2 * 1024 * 1024)
    throw new Error('This copy exceeds the 2 MiB save limit. Your editor draft is kept.');
  if (isTauri()) {
    const host = platform();
    // Mobile's existing transfer adapter exports verified saved receipts only.
    // These desktop editors are not composed in the mobile app; never claim
    // that adapter can preserve an arbitrary unsaved renderer buffer.
    if (host === 'ios' || host === 'android')
      throw new Error('Saving an unsaved copy is unavailable on this mobile build. Your editor draft is kept.');
    try {
      const reply = await unwrapNative(readerBackend().documentTransfer.readerExportDraft({ markdown, filename }));
      if (reply.state !== 'exported' && reply.state !== 'cancelled') throw new Error();
      return reply.state;
    } catch (cause) {
      const kind = cause && typeof cause === 'object' && 'kind' in cause ? cause.kind : null;
      if (kind === 'busy') throw new Error('Another save-copy dialog is open. Close it before retrying; your editor draft is kept.');
      if (kind === 'destination_exists_or_unavailable') throw new Error('Choose a new filename in a writable folder. Existing files are kept, and your editor draft is unchanged.');
      if (kind === 'too_large') throw new Error('This copy exceeds the 2 MiB save limit. Your editor draft is kept.');
      throw new Error('The copy could not be confirmed. Check the destination you chose before retrying; your editor draft is kept.');
    }
  }
  const url = URL.createObjectURL(new Blob([markdown], { type: 'text/markdown;charset=utf-8' }));
  try {
    const anchor = document.createElement('a');
    anchor.href = url; anchor.download = filename; anchor.click();
  } finally { window.setTimeout(() => URL.revokeObjectURL(url), 0); }
  return 'downloaded';
}
