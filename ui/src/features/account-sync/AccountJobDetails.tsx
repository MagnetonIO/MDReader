import { accountError, type AccountJob } from './backend.js';

export function AccountJobDetails({ job }: { job: AccountJob | null | undefined }) {
  if (!job) return null;
  const outcome = job.state.state === 'completed' ? job.state.outcome : null;
  return (
    <div aria-label="Latest account operation">
      {job.selectedSync ? <details><summary>Selected sync identity</summary>
        <p>Document: {job.selectedSync.receipt.documentId}. Revision: {job.selectedSync.receipt.revision}. Request: {job.selectedSync.operationId}.</p>
      </details> : null}
      {job.state.state === 'running' ? (
        <p role="status">Operation: {job.state.phase.replaceAll('_', ' ')}.</p>
      ) : null}
      {job.state.state === 'failed' ? <p role="alert">{accountError(job.state.error)}</p> : null}
      {job.state.state === 'cancelled' ? (
        <p>Operation cancelled; previously durable state remains retained.</p>
      ) : null}
      {outcome?.kind === 'connected' ? <p>Native account authority connected.</p> : null}
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
      {outcome?.kind === 'custody_stored' ? <p>Native secure custody stored.</p> : null}
      {outcome?.kind === 'handoff_complete' ? <p>Public device handoff recorded.</p> : null}
      {outcome?.kind === 'public_packet' ? (
        <label>
          Public {outcome.packetKind} packet
          <textarea readOnly value={outcome.packet} aria-label="Public device packet" />
        </label>
      ) : null}
    </div>
  );
}
