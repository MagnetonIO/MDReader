/** Native owns destination selection, freeze, backup and verification. */
import { useCallback, useEffect, useRef, useState, useSyncExternalStore } from 'react';
import { isTauri } from '../../shell/persistence';
import { unwrapNative, useReaderBackend } from '../../platform/backend';
import { editorAdmission } from '../editor-preservation/admission';
import { importRequest, type ImportReply } from '../profile-import/backend';
import { ProfileImport } from '../profile-import/ProfileImport';

const failures: Record<string, string> = {
  busy: 'The backup could not be confirmed. Your originals and drafts were kept. If editing stays paused, copy your drafts before restarting the app.',
  offline_export_required: 'A complete live backup is unavailable for this library format. Keep the originals. Use Import reader data to check what can be brought into a separate library.',
  too_large: 'This library exceeds the backup limits. Your originals and drafts were kept.',
  unavailable: 'A complete backup is unavailable on this device. Your originals and drafts were kept.',
  backup_unconfirmed: 'The backup could not be confirmed. Keep your originals; a partial copy may remain in the folder you chose. If editing stays paused, copy your drafts before restarting the app.',
};
export function BackupSettings({ returnController = false, resumeReturnOnOpen = false, onReturnResumeStarted }: {
  returnController?: boolean; resumeReturnOnOpen?: boolean; onReturnResumeStarted?(): void;
}) {
  const backend = useReaderBackend();
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [failure, setFailure] = useState<string | null>(null);
  const [selection, setSelection] = useState<ImportReply | null>(null);
  const mounted = useRef(true);
  const running = useRef(false);
  const importButton = useRef<HTMLButtonElement>(null);
  const hadSelection = useRef(false);
  const returnRequested = useRef(false);
  const held = useSyncExternalStore(editorAdmission.subscribe, editorAdmission.denied, () => false);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; };
  }, []);
  useEffect(() => {
    if (selection) hadSelection.current = true;
    else if (hadSelection.current) {
      hadSelection.current = false;
      importButton.current?.focus();
    }
  }, [selection]);
  async function backup() {
    if (running.current || held) return;
    running.current = true;
    setBusy(true); setNotice(null); setFailure(null);
    try {
      const result = await unwrapNative(backend.profile.nativeProfileBackup());
      if (result.state === 'exported') setNotice('Your reader data and unsaved drafts were backed up. Your originals were kept.');
      else if (result.state === 'cancelled') setNotice('Backup cancelled. Your originals were kept.');
      else setFailure(failures.backup_unconfirmed!);
    } catch (cause) {
      const kind = cause && typeof cause === 'object' && 'kind' in cause ? String(cause.kind) : '';
      setFailure(failures[kind] ?? failures.backup_unconfirmed!);
    } finally { running.current = false; setBusy(false); }
  }
  const chooseImport = useCallback(async (operation: 'preview' | 'resume' | 'resume_return') => {
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
  }, [held, selection]);
  useEffect(() => {
    if (!returnController || !resumeReturnOnOpen || returnRequested.current || held || selection || running.current) return;
    returnRequested.current = true;
    onReturnResumeStarted?.();
    void chooseImport('resume_return');
  }, [returnController, resumeReturnOnOpen, onReturnResumeStarted, held, selection, chooseImport]);
  return <section className="settings-section" aria-labelledby="reader-backup-title">
    <h3 id="reader-backup-title">{returnController ? 'Return to original library' : 'Back up reader data'}</h3>
    {!returnController ? <>
      <p>Choose a folder for a new copy of your local reader data and unsaved drafts. Cloud credentials stay on this device. Your originals are kept.</p>
      <p>Keep backing up Markdown folders you manage separately.</p>
    </> : null}
    <div className="settings-account-actions">
      {!returnController ? <><button type="button" className="settings-link-button" disabled={!isTauri() || busy || held || selection !== null} onClick={() => { void backup(); }}>
        Back up reader data
      </button>
      <button ref={importButton} type="button" className="settings-link-button" disabled={!isTauri() || busy || held || selection !== null} onClick={() => { void chooseImport('preview'); }}>Import reader data…</button></> : null}
      <button type="button" className="settings-link-button" disabled={!isTauri() || busy || held || selection !== null} onClick={() => { void chooseImport(returnController ? 'resume_return' : 'resume'); }}>{returnController ? 'Continue return…' : 'Continue an import…'}</button>
    </div>
    {!returnController ? <p>Continue an import to open a prepared library or return to its original library.</p> : null}
    {busy ? <p role="status">{returnController ? 'Checking the pending return…' : 'Choose a folder in the native dialog, then wait for preparation to finish.'}</p> : null}
    {selection ? <ProfileImport initial={selection} returnController={returnController} onClose={() => setSelection(null)} /> : null}
    {!isTauri() ? <p>Reader backups require the native desktop app.</p> : null}
    {held && !busy ? <p role="status">Preparation is in progress. Keep this window open; your drafts are retained.</p> : null}
    {notice ? <p role="status">{notice}</p> : null}
    {failure ? <p role="alert">{failure}</p> : null}
  </section>;
}
