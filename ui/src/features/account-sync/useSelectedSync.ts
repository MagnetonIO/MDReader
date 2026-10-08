import { useCallback, useEffect, useRef, useState } from 'react';
import type { DeviceReference } from '../device-reader/backend';
import type { useAccountSync } from './useAccountSync';
import { pendingUploads, retainUpload, sameScope, sameSelection, sameUpload, settleUpload, type PendingUpload, type SelectedPurpose } from './pending-selection';

type Account = ReturnType<typeof useAccountSync>;
/** One retained opaque request per verified native scope. Observations never upload. */
export function useSelectedSync(account: Account, selected: DeviceReference | undefined) {
  const [pending, setPending] = useState<PendingUpload | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [otherScope, setOtherScope] = useState(false);
  const dispatching = useRef(false);
  const scope = account.status?.selectedScope;
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
    if (jobPurpose !== 'content' && jobPurpose !== 'user_state') return;
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
          setNotice(outcome.disposition.state === 'applied'
            ? jobPurpose === 'user_state' ? 'The saved pin and reading position for this document were applied.'
              : 'This exact document upload was applied. Other documents are unchanged.'
            : 'The native owner confirmed this request was not admitted. You can choose a document again.');
        } else {
          setNotice(outcome.disposition.state === 'reauthorization_required'
            ? 'This upload is retained from an earlier sign-in. Resuming its authority is not available yet.'
            : 'This exact upload is queued. Retry keeps the same encrypted operation.');
        }
      }
      load();
    } catch (cause) { if (!cancelled) setError(cause instanceof Error ? cause.message : 'The exact upload identity remains unresolved.'); } };
    void reconcile();
    return () => { cancelled = true; };
  }, [observed, scope, load]);
  const start = useCallback(async (inspect: boolean, purpose: SelectedPurpose = 'content') => {
    if (dispatching.current || account.busy || account.running || !scope) return;
    dispatching.current = true;
    setNotice(null);
    try {
      const exact = pending ?? (!inspect && selected ? {
        purpose,
        selection: { scope, operationId: crypto.randomUUID(),
          receipt: { documentId: selected.documentId, sourceId: selected.sourceId, revision: selected.revision } },
      } : null);
      if (!exact || !sameScope(exact.selection.scope, scope)) return;
      // Persist before ANY dispatch. Selection change/unmount/error/cancel does
      // not clear it. Only a later exact readonly disposition may settle it.
      await retainUpload(exact);
      if (!mounted.current) return;
      if (!currentScope.current || !sameScope(exact.selection.scope, currentScope.current)) {
        load();
        setNotice('The account changed before dispatch. This exact request was retained for its original account.');
        return;
      }
      setPending(exact);
      setError(null);
      const operation = exact.purpose === 'user_state'
        ? inspect ? 'inspect_selected_state' : 'sync_selected_state'
        : inspect ? 'inspect_selected' : 'sync_selected';
      await account.begin({ operation, selection: exact.selection });
    } catch (cause) { if (mounted.current) setError(cause instanceof Error ? cause.message : 'The upload identity could not be preserved.'); }
    finally { dispatching.current = false; }
  }, [account.begin, account.busy, account.running, scope, pending, selected, load]);
  return { pending, error, notice, otherScope, upload: (purpose?: SelectedPurpose) => start(false, purpose), inspect: () => start(true) };
}
