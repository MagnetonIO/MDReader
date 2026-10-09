/** Native Settings composition; backend operations retain their existing authority checks. */
import { useCallback, useLayoutEffect, useRef, useState } from 'react';
import { AccountSyncPanel } from '../features/account-sync/AccountSyncPanel.js';
import { AgentCapturePanel } from './AgentCapturePanel.js';
import { IndexingPanel } from './IndexingPanel.js';
import { NativeProvidersPanel } from './NativeProvidersPanel.js';
import { CloseIcon } from './icons.js';
import { AppearanceSettings } from '../features/settings/AppearanceSettings.js';
import { ImportedHistory } from '../features/imported-history/ImportedHistory';
import { BackupSettings } from '../features/settings/BackupSettings.js';
import { LibrarySettings } from '../features/settings/LibrarySettings.js';
import { DiagnosticSettings } from '../features/settings/DiagnosticSettings.js';
import { ReadingLinkOpen } from '../features/document-transfer/ReadingLinkOpen.js';
import { SettingsConfirmations } from '../features/settings/SettingsConfirmations.js';
import { useSettingsController } from '../features/settings/useSettingsController.js';
import { visibleSettingsControls } from '../features/settings/focus.js';
import type { SettingsSheetProps } from '../features/settings/contracts.js';
import '../features/settings/settings.css';

export type { SettingsSheetProps } from '../features/settings/contracts.js';

const categories = [
  ['appearance', 'Appearance'],
  ['library', 'Library & sources'],
  ['account', 'Account & sync'],
  ['backup', 'Backup & storage'],
  ['advanced', 'Advanced'],
] as const;
type Category = (typeof categories)[number][0];

export function SettingsSheet(props: SettingsSheetProps) {
  const controller = useSettingsController(props);
  const { dialogRef, closeRef, confirmDialogRef, confirmClear, confirmForget,
    setConfirmClear, setConfirmForget, failure, notice, onClose,
    onOpenProviderDocument, onLibraryChanged, localRoot } = controller;
  const [category, setCategory] = useState<Category>(props.initialSection ?? 'appearance');
  const contentRef = useRef<HTMLDivElement>(null);
  useLayoutEffect(() => {
    if (contentRef.current) contentRef.current.scrollTop = 0;
  }, [category]);
  const confirming = confirmClear || confirmForget !== null;

  const onKeyDown = useCallback((event: React.KeyboardEvent<HTMLDivElement>) => {
    // The native dialog's own focus and Cancel lifecycle govern historical matching.
    if (event.target instanceof Element && event.target.closest('dialog[open]')) return;
    if (event.key === 'Escape') {
      event.preventDefault();
      event.stopPropagation();
      if (confirmForget) setConfirmForget(null);
      else if (confirmClear) setConfirmClear(false);
      else onClose();
      return;
    }
    if (event.key !== 'Tab') return;
    const scope = confirming ? confirmDialogRef.current : dialogRef.current;
    const controls = visibleSettingsControls(scope);
    const first = controls[0];
    const last = controls.at(-1);
    if (!first || !last) return;
    if (!event.shiftKey && (document.activeElement === last || !scope?.contains(document.activeElement))) {
      event.preventDefault();
      first.focus();
    } else if (event.shiftKey && (document.activeElement === first || !scope?.contains(document.activeElement))) {
      event.preventDefault();
      last.focus();
    }
  }, [confirmClear, confirmForget, confirming, confirmDialogRef, dialogRef, onClose, setConfirmClear, setConfirmForget]);

  function selectCategory(next: Category) {
    setCategory(next);
  }

  return (
    <div className="settings-backdrop" role="presentation" onMouseDown={onClose}>
      <div ref={dialogRef} className="settings-sheet settings-sheet--categorized"
        role="dialog" aria-modal="true" aria-labelledby="settings-title"
        onKeyDown={onKeyDown} onMouseDown={(event) => event.stopPropagation()}>
        <header className="settings-sheet__header" inert={confirming}>
          <h1 id="settings-title" className="settings-sheet__title">Settings</h1>
          <button ref={closeRef} type="button" className="settings-sheet__close"
            aria-label="Close Settings" onClick={onClose}><CloseIcon /></button>
        </header>
        <div className="settings-layout" inert={confirming}>
          <label className="settings-mobile-navigation">
            Settings section
            <select aria-label="Settings section" value={category} onChange={(event) => selectCategory(event.currentTarget.value as Category)}>
              {categories.map(([id, label]) => <option key={id} value={id}>{label}</option>)}
            </select>
          </label>
          <nav className="settings-navigation" aria-label="Settings sections">
            {categories.map(([id, label]) => (
              <button key={id} type="button" aria-current={category === id ? 'page' : undefined}
                aria-controls={`settings-category-${id}`} onClick={() => selectCategory(id)}>{label}</button>
            ))}
          </nav>
          <div ref={contentRef} className="settings-sheet__scroll settings-content">
            <div id="settings-category-appearance" hidden={category !== 'appearance'}><AppearanceSettings /></div>
            <div id="settings-category-library" hidden={category !== 'library'}>
              <LibrarySettings controller={controller} />
              {onOpenProviderDocument ? (
                <NativeProvidersPanel profileKey="native-library" onOpen={onOpenProviderDocument} onSourcesChanged={onLibraryChanged} />
              ) : <p>Cloud sources are unavailable in this context.</p>}
              {props.onOpenReadingLink ? <ReadingLinkOpen onOpen={props.onOpenReadingLink} /> : null}
            </div>
            <section id="settings-category-account" hidden={category !== 'account'} aria-labelledby="settings-account-title">
              <h2 id="settings-account-title">Account &amp; sync</h2>
              <AccountSyncPanel active={category === 'account'} selectedDocument={props.selectedDevice} />
            </section>
            <section id="settings-category-backup" hidden={category !== 'backup'} aria-labelledby="settings-backup-title">
              <h2 id="settings-backup-title">Backup &amp; storage</h2>
              <p>Local Markdown originals stay in the folders you selected. Private indexes can be rebuilt; they are not document backups.</p>
              <BackupSettings returnController={props.returnController} resumeReturnOnOpen={props.resumeReturnOnOpen}
                onReturnResumeStarted={props.onReturnResumeStarted} />
              {category === 'backup' && props.onOpenImportedDocument ? (
                <ImportedHistory onOpen={props.onOpenImportedDocument} onReconnectFolder={props.onReconnectImportedFolder} />
              ) : null}
              <h3>Manage local copies</h3>
              <p>Use Library &amp; sources to manage folders and selected cloud copies. Removing a folder from MDReader leaves its originals untouched.</p>
              <button type="button" className="settings-link-button" onClick={() => selectCategory('library')}>Manage library sources</button>
            </section>
            <section id="settings-category-advanced" hidden={category !== 'advanced'} aria-labelledby="settings-advanced-title">
              <h2 id="settings-advanced-title">Advanced</h2>
              <IndexingPanel />
              <AgentCapturePanel root={localRoot} />
              <DiagnosticSettings controller={controller} />
            </section>
          </div>
        </div>
        {failure ? <div className="settings-sheet__message settings-sheet__message--error" role="alert">{failure}</div> : null}
        {notice ? <div className="settings-sheet__message" role="status">{notice}</div> : null}
        <SettingsConfirmations controller={controller} />
      </div>
    </div>
  );
}
