/** Native owns destination selection, freeze, backup and verification. */
import { useState, useSyncExternalStore } from 'react';
import { invoke } from '@tauri-apps/api/core';
import { isTauri } from '../../shell/persistence';
import { editorAdmission } from '../editor-preservation/admission';

const failures: Record<string, string> = {
  busy: 'The backup could not be confirmed. Your originals and drafts were kept. If editing stays paused, copy your drafts before restarting the app.',
  offline_export_required: 'This library contains older or unsupported data. Keep the originals and use the offline export tool before moving it.',
  too_large: 'This library exceeds the backup limits. Your originals and drafts were kept.',
  unavailable: 'A complete backup is unavailable on this device. Your originals and drafts were kept.',
  backup_unconfirmed: 'The backup could not be confirmed. Keep your originals; a partial copy may remain in the folder you chose. If editing stays paused, copy your drafts before restarting the app.',
};
export function BackupSettings() {
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const held = useSyncExternalStore(editorAdmission.subscribe, editorAdmission.denied, () => false);
  async function backup() {
    if (busy || held) return;
    setBusy(true); setNotice(null); setFailure(null);
    try {
      const result = await invoke<{ state: 'exported' | 'cancelled' }>('native_profile_backup');
      if (result.state === 'exported') setNotice('Your reader data and unsaved drafts were backed up. Your originals were kept.');
      else if (result.state === 'cancelled') setNotice('Backup cancelled. Your originals were kept.');
      else setFailure(failures.backup_unconfirmed!);
    } catch (cause) {
      const kind = cause && typeof cause === 'object' && 'kind' in cause ? String(cause.kind) : '';
      setFailure(failures[kind] ?? failures.backup_unconfirmed!);
    } finally { setBusy(false); }
  }
  return <section className="settings-section" aria-labelledby="reader-backup-title">
    <h3 id="reader-backup-title">Back up reader data</h3>
    <p>Choose a folder for a new copy of your local reader data and unsaved drafts. Cloud credentials stay on this device. Your originals are kept.</p>
    <p>Keep backing up Markdown folders you manage separately. Restore is not available in this panel.</p>
    <button type="button" className="settings-link-button" disabled={!isTauri() || busy || held} onClick={() => { void backup(); }}>
      {busy ? 'Backing up…' : 'Back up reader data'}
    </button>
    {!isTauri() ? <p>Reader backups require the native desktop app.</p> : null}
    {held && !busy ? <p role="status">Preparation is in progress. Keep this window open; your drafts are retained.</p> : null}
    {notice ? <p role="status">{notice}</p> : null}
    {failure ? <p role="alert">{failure}</p> : null}
  </section>;
}
