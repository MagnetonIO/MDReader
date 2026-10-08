import type { DeviceReference } from '../device-reader/backend';
import type { useAccountSync } from './useAccountSync';
import { useSelectedSync } from './useSelectedSync';

export function SelectedDocumentSync({ account, selected }: {
  account: ReturnType<typeof useAccountSync>; selected?: DeviceReference;
}) {
  const upload = useSelectedSync(account, selected);
  const disabled = account.busy || account.running || account.status?.latestJob?.state.state === 'running'
    || !account.status?.connected || !account.status.selectedScope;
  return <div className="settings-sync-selection" aria-label="Selected document sync">
    <h4>Sync a document</h4>
    <p>{selected ? `Selected: ${selected.title || 'Untitled document'}.` : 'Open a document from On this device to select it for sync.'}</p>
    {upload.error ? <p role="alert">{upload.error}</p> : null}
    {upload.notice ? <p role="status">{upload.notice}</p> : null}
    {upload.otherScope ? <p>{account.status?.selectedScope
      ? 'An upload identity for another account or library is retained. It will not be sent to this account.'
      : 'A retained upload is waiting for its original account and library. Sign in before checking it.'}</p> : null}
    {upload.pending ? <>
      <p>A dispatched upload is retained. Changing the selected document does not replace it.</p>
      <details><summary>Retained upload identity</summary><p>
        Document: {upload.pending.receipt.documentId}. Revision: {upload.pending.receipt.revision}.
      </p></details>
      <div className="settings-account-actions">
        <button type="button" disabled={disabled} onClick={() => void upload.inspect()}>Check retained upload</button>
        <button type="button" disabled={disabled || !!upload.error} onClick={() => void upload.upload()}>Retry retained upload</button>
      </div>
    </> : <button type="button" className="settings-account-primary" disabled={disabled || !selected || !!upload.error}
      onClick={() => void upload.upload()}>Sync this document</button>}
    <p>Uploads include this document and its tracked images. Pin and reading-position changes are not uploaded yet.</p>
  </div>;
}
