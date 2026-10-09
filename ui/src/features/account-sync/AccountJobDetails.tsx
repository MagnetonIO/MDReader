import { accountError, type AccountJob } from './backend.js';
import { PublicPacketExport } from './PublicPacketExport';

const phaseText: Record<string, string> = {
  signing_in: 'Finish signing in in your browser, then return to MDReader.',
  authorizing_device: 'Confirm the same account in your browser once more. This second confirmation authorizes this device to use your encrypted library.',
  preparing_enrollment: 'Preparing this device’s encrypted connection…',
  waiting_for_device_approval: 'Checking device approval…',
  reading_recovery: 'Choose your recovery file and enter its separate secret in the native dialog.',
  exporting_recovery: 'Choose where to save the encrypted recovery file in the native dialog.',
  syncing: 'Syncing the documents you explicitly queued…',
  queued: 'Your request is queued…',
};

export function AccountJobDetails({ job }: { job: AccountJob | null | undefined }) {
  if (!job) return null;
  const outcome = job.state.state === 'completed' ? job.state.outcome : null;
  return (
    <div aria-label="Latest account operation">
      {job.selectedSync ? <details><summary>Selected sync identity</summary>
        <p>Document: {job.selectedSync.receipt.documentId}. Revision: {job.selectedSync.receipt.revision}. Request: {job.selectedSync.operationId}.</p>
      </details> : null}
      {job.state.state === 'running' ? (
        <p role="status">{phaseText[job.state.phase] ?? 'Working on your connection. Keep this window open; your documents stay local.'}</p>
      ) : null}
      {job.state.state === 'failed' ? <p role="alert">{accountError(job.state.error)}</p> : null}
      {job.state.state === 'cancelled' ? (
        <p>Operation cancelled; previously durable state remains retained.</p>
      ) : null}
      {outcome?.kind === 'sync' ? (
        <>
        <p role="status">{outcome.pending ? 'A change needs attention before sync can continue. Your local copies are kept.'
          : outcome.remotePending || outcome.localPending ? 'Some changes are still waiting to sync.' : 'Queued sync work finished.'}</p>
        <details><summary>Sync details</summary>
        <p>
          Committed uploads: {outcome.committed}. Conflicts: {outcome.conflicts}.
          Received: {outcome.received}. Applied: {outcome.applied}.
          Acknowledged: {outcome.acknowledged}.
          {outcome.remotePending ? ' Remote work remains pending.' : ''}
          {outcome.localPending ? ' Local work remains pending.' : ''}
          {outcome.pending ? ` Incoming item ${outcome.pending.sequence}: ${outcome.pending.reason.replaceAll('_', ' ')}.` : ''}
          {' '}These counts do not prove that every document is visible in the desktop reader.
        </p>
        </details>
        </>
      ) : null}
      {outcome?.kind === 'recovery_exported' ? (
        <p>Native recovery export completed. Store the file and its independently presented secret separately.</p>
      ) : null}
      {outcome?.kind === 'custody_stored' ? <p>Device keys were saved securely.</p> : null}
      {outcome?.kind === 'handoff_complete' ? <p>Device approval handoff confirmed.</p> : null}
      {outcome?.kind === 'public_packet' ? (
        <PublicPacketExport key={job.id} text={outcome.packet} kind={outcome.packetKind} />
      ) : null}
      <details className="settings-account-advanced"><summary>Advanced · operation details</summary><p>Operation ID: {job.id}. State: {job.state.state}.{job.state.state === 'running' ? ` Phase: ${job.state.phase}.` : ''}</p></details>
    </div>
  );
}
