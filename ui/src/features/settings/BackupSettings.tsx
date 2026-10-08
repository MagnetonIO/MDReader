/** Native owns destination selection, freeze, backup and verification. */
import { useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { isTauri } from '../../shell/persistence';
import { editorAdmission } from '../editor-preservation/admission';
import { importRequest, type ImportReply } from '../profile-import/backend';
import { ProfileImport } from '../profile-import/ProfileImport';

const failures: Record<string, string> = {
  busy: 'The backup could not be confirmed. Your originals and drafts were kept. If editing stays paused, copy your drafts before restarting the app.',
  offline_export_required: 'This library contains older or unsupported data. Keep the originals; they require an offline export before moving this library.',
  too_large: 'This library exceeds the backup limits. Your originals and drafts were kept.',
  unavailable: 'A complete backup is unavailable on this device. Your originals and drafts were kept.',
  backup_unconfirmed: 'The backup could not be confirmed. Keep your originals; a partial copy may remain in the folder you chose. If editing stays paused, copy your drafts before restarting the app.',
};
export function BackupSettings() {
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [selection, setSelection] = useState<ImportReply | null>(null);
  const mounted = useRef(true);
  const running = useRef(false);
  const held = useSyncExternalStore(editorAdmission.subscribe, editorAdmission.denied, () => false);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  async function backup() {
    if (running.current || held) return;
    running.current = true;
    setBusy(true); setNotice(null); setFailure(null);
    try {
      const result = await invoke<{ state: 'exported' | 'cancelled' }>('native_profile_backup');
      if (result.state === 'exported') setNotice('Your reader data and unsaved drafts were backed up. Your originals were kept.');
      else if (result.state === 'cancelled') setNotice('Backup cancelled. Your originals were kept.');
      else setFailure(failures.backup_unconfirmed!);
    } catch (cause) {
      const kind = cause && typeof cause === 'object' && 'kind' in cause ? String(cause.kind) : '';
      setFailure(failures[kind] ?? failures.backup_unconfirmed!);
    } finally { running.current = false; setBusy(false); }
  }
  async function chooseImport(operation: 'preview' | 'resume') {
    if (running.current || held || selection) return;
    running.current = true;
    setBusy(true); setNotice(null); setFailure(null);
    try {
      const value = await importRequest({ operation });
      if (!mounted.current) {
        if (value.operationId) void importRequest({ operation: 'close', operationId: value.operationId }).catch(() => {});
        return;
      }
      if (value.state !== 'cancelled') setSelection(value);
    } catch (cause) {
      if (mounted.current) setFailure(cause instanceof Error ? cause.message : 'The selected library could not be opened. Your originals were kept.');
    } finally {
      running.current = false;
      if (mounted.current) setBusy(false);
    }
  }
  return <section className="settings-section" aria-labelledby="reader-backup-title">
    <h3 id="reader-backup-title">Back up reader data</h3>
    <p>Choose a folder for a new copy of your local reader data and unsaved drafts. Cloud credentials stay on this device. Your originals are kept.</p>
    <p>Keep backing up Markdown folders you manage separately.</p>
    <div className="settings-account-actions">
      <button type="button" className="settings-link-button" disabled={!isTauri() || busy || held || selection !== null} onClick={() => { void backup(); }}>
        Back up reader data
      </button>
      <button type="button" className="settings-link-button" disabled={!isTauri() || busy || held || selection !== null} onClick={() => { void chooseImport('preview'); }}>Import reader data…</button>
      <button type="button" className="settings-link-button" disabled={!isTauri() || busy || held || selection !== null} onClick={() => { void chooseImport('resume'); }}>Continue an import…</button>
    </div>
    {busy ? <p role="status">Choose a folder in the native dialog, then wait for preparation to finish.</p> : null}
    {selection ? <ProfileImport initial={selection} onClose={() => setSelection(null)} /> : null}
    {!isTauri() ? <p>Reader backups require the native desktop app.</p> : null}
    {held && !busy ? <p role="status">Preparation is in progress. Keep this window open; your drafts are retained.</p> : null}
    {notice ? <p role="status">{notice}</p> : null}
    {failure ? <p role="alert">{failure}</p> : null}
  </section>;
}
