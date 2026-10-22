import { useEffect, useId, useRef } from 'react';
import type { NativeProviderKind, NativeProvidersSnapshot } from '../../platform/providers.js';
import { ActionMenu } from '../providers/ActionMenu.js';
import { ProviderIcon } from '../providers/ProviderIcon.js';
import { providerName } from '../providers/source-presentation.js';
import type { NativeProvidersController } from '../providers/useNativeProviders.js';

const kinds: NativeProviderKind[] = ['google_drive', 'dropbox'];

/** Unavailable providers stay listed, disabled, with the native reason. */
export function AddSourceMenu({ snapshot, canConnect, onChoose }: {
  snapshot: NativeProvidersSnapshot | null; canConnect(kind: NativeProviderKind): boolean; onChoose(kind: NativeProviderKind): void;
}) {
  return <ActionMenu label="Add source" className="provider-add-menu" items={kinds.map(kind => {
    const available = canConnect(kind);
    const diagnostic = snapshot?.availability.find(value => value.provider === kind)?.diagnostic;
    return { id: kind, label: providerName(kind), icon: <ProviderIcon provider={kind} />, disabled: !available,
      description: available ? undefined : diagnostic ?? 'Unavailable in this version of MDReader', onSelect: () => onChoose(kind) };
  })} />;
}

/** Connection options only; native custody remains responsible for authorization. */
export function AddSourcePanel({ kind, controller, allowEditing }: {
  kind: NativeProviderKind; controller: NativeProvidersController; allowEditing: boolean;
}) {
  const heading = useRef<HTMLHeadingElement>(null);
  const titleId = useId();
  const { form, busy } = controller;
  const available = controller.canConnect(kind);
  useEffect(() => { heading.current?.focus(); }, [kind]);
  return <section className="provider-add" aria-labelledby={titleId}>
    <h3 id={titleId} ref={heading} tabIndex={-1} className="provider-add__title"><ProviderIcon provider={kind} />Add {providerName(kind)}</h3>
    {kind === 'google_drive' ? <DriveOptions controller={controller} allowEditing={allowEditing} /> : <DropboxOptions controller={controller} allowEditing={allowEditing} />}
    <div className="provider-add__actions">
      <button type="button" className="provider-button--primary" disabled={busy || !available || kind === 'dropbox' && !form.pathValid}
        onClick={() => controller.connect(kind)}>{kind === 'google_drive' ? 'Connect Google Drive' : 'Connect Dropbox folder'}</button>
      <button type="button" onClick={() => controller.setAdding(null)}>Cancel</button>
    </div>
    {kind === 'google_drive' ? <p className="provider-add__hint">
      Need access to only a few files?{' '}
      <button type="button" className="provider-button--link" disabled={busy || !available} onClick={controller.connectGoogleFiles}>Choose specific files instead</button>
    </p> : null}
  </section>;
}

function DriveOptions({ controller, allowEditing }: { controller: NativeProvidersController; allowEditing: boolean }) {
  const { form, busy } = controller;
  return <fieldset className="provider-add__options" disabled={busy}>
    <legend className="provider-add__legend">New Google Drive connection options</legend>
    <p className="provider-add__hint">MDReader lists .md and .markdown files in My Drive and downloads a file only when you open it.</p>
    <Check label="Include subfolders" checked={form.driveRecursive} onChange={form.setDriveRecursive} />
    {allowEditing ? <Check label="Allow editing" checked={form.driveEditing} onChange={form.setDriveEditing} /> : null}
    <p className="provider-add__hint">{allowEditing && form.driveEditing
      ? 'Google grants read and write access. MDReader edits only Markdown in your selected folders and saves only over an unchanged version.'
      : 'Read only. Existing connections keep their permissions.'}</p>
  </fieldset>;
}

function DropboxOptions({ controller, allowEditing }: { controller: NativeProvidersController; allowEditing: boolean }) {
  const { form, busy } = controller;
  const pathId = useId();
  const pathHintId = useId();
  return <fieldset className="provider-add__options" disabled={busy}>
    <legend className="provider-add__legend">Dropbox folder options</legend>
    <label className="provider-add__field" htmlFor={pathId}>Dropbox folder</label>
    <input id={pathId} aria-label="Dropbox folder path" aria-describedby={pathHintId} aria-invalid={!form.pathValid} value={form.dropboxPath}
      onChange={(event) => form.setDropboxPath(event.target.value)} maxLength={4096} spellCheck={false} />
    <p id={pathHintId} className="provider-add__hint">{form.pathValid ? 'Use a path such as /Notes. / includes your whole accessible Dropbox.' : 'Enter a path starting with /, without . or .. folders.'}</p>
    <Check label="Include subfolders" checked={form.recursive} onChange={form.setRecursive} />
    {allowEditing ? <Check label="Allow editing" checked={form.dropboxEditing} onChange={form.setDropboxEditing} /> : null}
    <p className="provider-add__hint">{allowEditing && form.dropboxEditing ? 'Dropbox will ask for write permission.' : 'Read only.'} To change the folder or permissions later, add a new connection.</p>
  </fieldset>;
}

function Check({ label, checked, onChange }: { label: string; checked: boolean; onChange(value: boolean): void }) {
  return <label className="provider-add__check"><input type="checkbox" checked={checked} onChange={event => onChange(event.target.checked)} /><span>{label}</span></label>;
}
