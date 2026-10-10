import { invoke } from '@tauri-apps/api/core';
import type { CanonicalReaderReference } from '../reader/contracts';
import { receipt } from '../reader-history/references';
import { documentFilename } from './filename';

/** Native owner verifies the actual current body; the save dialog owns the
 * destination. Renderer input carries neither a body nor an output path. */
export async function exportNativeMarkdown(reference: CanonicalReaderReference): Promise<'exported' | 'cancelled'> {
  const exact = receipt(reference);
  try {
    const reply = await invoke<{ state: string }>('reader_export_document', { reference: {
      sourceId: exact.sourceId, revision: exact.revision,
      path: exact.kind === 'device' ? 'document.md' : exact.path,
      documentId: exact.kind === 'device' ? exact.documentId : null,
      filename: documentFilename(reference),
    } });
    if (reply.state !== 'exported' && reply.state !== 'cancelled') throw new Error();
    return reply.state;
  } catch (cause) {
    const kind = cause && typeof cause === 'object' && 'kind' in cause ? cause.kind : null;
    if (kind === 'stale' || kind === 'conflict') throw new Error('The document revision changed. Open its exact current receipt before exporting.');
    if (kind === 'unauthorized') throw new Error('This source is unavailable or revoked. No copy was exported.');
    if (kind === 'unsupported') throw new Error('Native file export is unavailable on this platform. The source is unchanged.');
    if (kind === 'too_large' || kind === 'limit') throw new Error('This document exceeds the supported export budget. The source is unchanged.');
    throw new Error('Export could not be confirmed. Check the destination you selected; the source is unchanged.');
  }
}
