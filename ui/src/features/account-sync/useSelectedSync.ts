import { useCallback, useEffect, useRef, useState } from 'react';
import { notifyDeviceReadChanged } from '../device-reader/changed';
import type { DeviceReference } from '../device-reader/backend';
import type { SelectedScope } from './backend';
import type { useAccountSync } from './useAccountSync';
import { pendingUploads, retainUpload, sameScope, sameSelection, sameUpload, settleUpload, type PendingUpload, type SelectedPurpose } from './pending-selection';

export interface DeletionSelection { reference: DeviceReference; scope: SelectedScope }
const sameDocument = (a: DeviceReference | undefined, b: DeviceReference) => a?.documentId === b.documentId && a.sourceId === b.sourceId && a.revision === b.revision;
type Account = ReturnType<typeof useAccountSync>;
/** One retained opaque request per verified native scope. Observations never upload. */
export function useSelectedSync(account: Account, selected: DeviceReference | undefined) {
  const [pending, setPending] = useState<PendingUpload | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [otherScope, setOtherScope] = useState(false);
  const dispatching = useRef(false);
  const scope = account.status?.selectedScope;
  const currentSelection = useRef(selected);
  currentSelection.current = selected;
  const currentScope = useRef(scope);
  currentScope.current = scope;
  const mounted = useRef(true);
  useEffect(() => { mounted.current = true; return () => { mounted.current = false; }; }, []);
  const load = useCallback(() => {
    try {
      const all = pendingUploads();
      setPending(scope ? all.find((op) => sameScope(op.selection.scope, scope)) ?? null : null);
      setOtherScope(all.some((op) => !scope || !sameScope(op.selection.scope, scope)));
      setError(null);
    } catch (cause) { setError(cause instanceof Error ? cause.message : 'The retained upload identity could not be read.'); }
  }, [scope]);
  useEffect(load, [load]);
  useEffect(() => {
    window.addEventListener('storage', load);
    return () => window.removeEventListener('storage', load);
  }, [load]);
  const observed = account.status?.latestJob;
  useEffect(() => {
    const identity = observed?.selectedSync;
    if (!observed || !identity || !scope || !sameScope(identity.scope, scope)) return;
    const jobPurpose = (observed as typeof observed & { selectedPurpose?: SelectedPurpose | null }).selectedPurpose ?? 'content';
    if (jobPurpose !== 'content' && jobPurpose !== 'user_state' && jobPurpose !== 'delete') return;
    const exact: PendingUpload = { purpose: jobPurpose, selection: identity };
    let cancelled = false;
    const reconcile = async () => { try {
      const retained = pendingUploads().find((op) => sameScope(op.selection.scope, identity.scope));
      // An older native job must not replace a later dispatched request.
      if (retained && !sameUpload(retained, exact)) return;
      const outcome = observed.state.state === 'completed' ? observed.state.outcome : null;
      const purposeMatches = (outcome as typeof outcome & { purpose?: SelectedPurpose } | null)?.purpose === jobPurpose
        || jobPurpose === 'content' && !(outcome && 'purpose' in outcome);
      const settled = outcome?.kind === 'selected_status' && purposeMatches && sameSelection(outcome.selection, identity)
        && (outcome.disposition.state === 'absent' || outcome.disposition.state === 'applied');
      // Recover a lost begin reply, but never resurrect a settled inspection.
      if (!settled) await retainUpload(exact);
      if (cancelled) return;
      if (outcome?.kind === 'selected_status' && purposeMatches && sameSelection(outcome.selection, identity)) {
        if (outcome.disposition.state === 'absent' || outcome.disposition.state === 'applied') {
          if (retained) await settleUpload(exact);
          if (cancelled) return;
          if (outcome.disposition.state === 'applied' && jobPurpose === 'delete') notifyDeviceReadChanged(identity.receipt);
          setNotice(outcome.disposition.state === 'applied'
            ? jobPurpose === 'delete' ? 'This exact deletion was applied. The synced document has been retired.' : jobPurpose === 'user_state' ? 'The saved pin and reading position for this document were applied.'
              : 'This exact document upload was applied. Other documents are unchanged.'
            : 'The native owner confirmed this request was not admitted. You can choose a document again.');
        } else {
          setNotice(outcome.disposition.state === 'reauthorization_required'
            ? 'This request is retained from an earlier sign-in. Its original authority must be restored before proceeding.'
            : jobPurpose === 'delete' ? 'Deletion is queued. The local copy stays until the authenticated deletion is applied.' : 'This exact upload is queued. Retry keeps the same encrypted operation.');
        }
      }
      load();
    } catch (cause) { if (!cancelled) setError(cause instanceof Error ? cause.message : 'The exact upload identity remains unresolved.'); } };
    void reconcile();
    return () => { cancelled = true; };
  }, [observed, scope, load]);
  const start = useCallback(async (inspect: boolean, purpose: SelectedPurpose = 'content', deletion?: DeletionSelection) => {
    if (dispatching.current || account.busy || account.running || !scope) return;
    dispatching.current = true;
    setNotice(null);
    try {
      if (deletion && (pending || !sameScope(deletion.scope, scope) || !sameDocument(currentSelection.current, deletion.reference)))
        throw new Error('The selected document or account changed. Confirm deletion again; nothing was replaced.');
      const target = deletion?.reference ?? selected;
      const exact = pending ?? (!inspect && target ? {
        purpose,
        selection: { scope, operationId: crypto.randomUUID(),
          receipt: { documentId: target.documentId, sourceId: target.sourceId, revision: target.revision } },
      } : null);
      if (!exact || !sameScope(exact.selection.scope, scope)) return;
      // Persist before ANY dispatch. Selection change/unmount/error/cancel does
      // not clear it. Only a later exact readonly disposition may settle it.
      await retainUpload(exact);
      if (!mounted.current) return;
      if (!currentScope.current || !sameScope(exact.selection.scope, currentScope.current)
        || deletion && !sameDocument(currentSelection.current, deletion.reference)) {
        load();
        setNotice('The account changed before dispatch. This exact request was retained for its original account.');
        return;
      }
      setPending(exact);
      setError(null);
      const operation = exact.purpose === 'delete'
        ? inspect ? 'inspect_selected_delete' : 'sync_selected_delete'
        : exact.purpose === 'user_state'
        ? inspect ? 'inspect_selected_state' : 'sync_selected_state'
        : inspect ? 'inspect_selected' : 'sync_selected';
      await account.begin({ operation, selection: exact.selection });
    } catch (cause) { if (mounted.current) setError(cause instanceof Error ? cause.message : 'The upload identity could not be preserved.'); }
    finally { dispatching.current = false; }
  }, [account.begin, account.busy, account.running, scope, pending, selected, load]);
  return { pending, error, notice, otherScope, upload: (purpose?: Exclude<SelectedPurpose, 'delete'>) => start(false, purpose), remove: (deletion: DeletionSelection) => start(false, 'delete', deletion), inspect: () => start(true) };
}
