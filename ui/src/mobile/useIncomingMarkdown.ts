import { useEffect, useRef, useState } from 'react';
import type { MutableRefObject } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { listen } from '@tauri-apps/api/event';
import type { DeviceDocument } from '../bindings';

type IncomingFailure = 'invalid_selection' | 'unreadable_selection' | 'selection_limit'
  | 'import_unavailable' | 'queue_unavailable';
export type IncomingReply = {
  document: DeviceDocument | null;
  remaining: number;
  error: IncomingFailure | null;
  retained: boolean;
  open: boolean;
};
const failures: Record<IncomingFailure, string> = {
  invalid_selection: 'Choose a .md or .markdown file using Open with or Share.',
  unreadable_selection: 'A selected file could not be read as UTF-8 Markdown. Its original is unchanged.',
  selection_limit: 'Select up to 20 Markdown files, 1 MiB each and 8 MiB total.',
  import_unavailable: 'The selected copy is retained for retry. Check interrupted imports in Library if this continues.',
  queue_unavailable: 'The imported document is saved. Retry to finish clearing its pending selection.',
};
function verified(value: IncomingReply): IncomingReply {
  if (!value || !Number.isSafeInteger(value.remaining) || value.remaining < 0 || value.remaining > 21
    || typeof value.retained !== 'boolean' || typeof value.open !== 'boolean'
    || value.error !== null && !Object.hasOwn(failures, value.error)
    || value.retained && value.remaining === 0
    || !value.document && value.remaining > 0 && !value.error)
    throw new Error('The pending selection could not be verified.');
  if (value.document && (typeof value.document.id !== 'string' || !value.document.id
    || typeof value.document.source_id !== 'string' || !value.document.source_id
    || typeof value.document.revision !== 'string' || !value.document.revision
    || typeof value.document.name !== 'string')) throw new Error('The imported document could not be verified.');
  return value;
}
/** Native queue owns bytes and selected-URI authority. Only a published receipt
 * crosses to the UI; a failed/uncertain admission stops until explicit retry. */
export function useIncomingMarkdown(
  platform: 'ios' | 'android',
  navigation: MutableRefObject<number>,
  handlers: { imported: () => Promise<void>; open: (document: DeviceDocument) => Promise<void> },
) {
  const callbacks = useRef(handlers); callbacks.current = handlers;
  const retry = useRef<() => void>(() => {});
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [retained, setRetained] = useState(false);
  useEffect(() => {
    if (platform !== 'android') return;
    let current = true, running = false, woken = false, blocked = false, ready = false;
    let needsRefresh = false;
    let pendingOpen: { document: DeviceDocument; epoch: number } | null = null;
    let unlisten: (() => void) | undefined;
    const drain = async () => {
      if (!current || blocked) return;
      if (running) { woken = true; return; }
      running = true; woken = false;
      setBusy(true); setNotice(null); setError(null); setRetained(false);
      const epoch = navigation.current;
      const imported = new Map<string, DeviceDocument>();

      try {
        for (let count = 0; current && count < 21; count++) {
          const reply = verified(await invoke<IncomingReply>('mobile_reader_import_incoming'));
          if (!current) return;
          if (reply.document) {
            imported.set(reply.document.id, reply.document); needsRefresh = true;
            if (reply.open) pendingOpen = { document: reply.document, epoch };
          }
          if (reply.error) setError(failures[reply.error]);
          if (reply.retained) { blocked = true; setRetained(true); break; }
          if (!reply.remaining) break;
          if (count === 20) woken = true;
        }
        if (needsRefresh) {
          await callbacks.current.imported(); needsRefresh = false;
          if (!current) return;
          if (imported.size) setNotice(`${imported.size} file${imported.size === 1 ? '' : 's'} imported on this device.`);
          const selected = pendingOpen; pendingOpen = null;
          if (selected && navigation.current === selected.epoch) await callbacks.current.open(selected.document);
        }
      } catch {
        if (current) {
          blocked = true; setRetained(true);
          setError('The selected copy could not be confirmed. Retry to finish the import; your original is unchanged.');
        }
      } finally {
        running = false;
        if (current) {
          setBusy(false);
          if (woken && !blocked) void drain();
        }
      }
    };
    retry.current = () => { blocked = false; void drain(); };
    const foreground = () => { if (ready && document.visibilityState === 'visible') void drain(); };
    document.addEventListener('visibilitychange', foreground);
    // Install before cold-start drain, so a new selection cannot fall between
    // the first read and listener installation.
    void listen('mdreader://incoming-markdown', () => void drain()).then((stop) => {
      if (!current) { stop(); return; }
      unlisten = stop; ready = true; void drain();
    }).catch(() => { if (current) { ready = true; void drain(); } });
    return () => {
      current = false; retry.current = () => {};
      unlisten?.(); document.removeEventListener('visibilitychange', foreground);
    };
  }, [platform, navigation]);
  return { busy, error, notice, retained, retry: () => retry.current() };
}
