import { useCallback, useEffect, useRef, useState } from 'react';
import type { DeviceReference } from '../device-reader/backend';
import type { SelectedOperation } from '../../bindings';
import type { useAccountSync } from './useAccountSync';
import { pendingSelections, retainSelection, sameScope, sameSelection, settleSelection } from './pending-selection';

type Account = ReturnType<typeof useAccountSync>;
/** One retained opaque request per verified native scope. Observations never upload. */
export function useSelectedSync(account: Account, selected: DeviceReference | undefined) {
  const [pending, setPending] = useState<SelectedOperation | null>(null);
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
      const all = pendingSelections();
      setPending(scope ? all.find((op) => sameScope(op.scope, scope)) ?? null : null);
      setOtherScope(all.some((op) => !scope || !sameScope(op.scope, scope)));
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
    let cancelled = false;
    const reconcile = async () => { try {
      const retained = pendingSelections().find((op) => sameScope(op.scope, identity.scope));
      // An older native job must not replace a later dispatched request.
      if (retained && !sameSelection(retained, identity)) return;
      const outcome = observed.state.state === 'completed' ? observed.state.outcome : null;
      const settled = outcome?.kind === 'selected_status' && sameSelection(outcome.selection, identity)
        && (outcome.disposition.state === 'absent' || outcome.disposition.state === 'applied');
      // Recover a lost begin reply, but never resurrect a settled inspection.
      if (!settled) await retainSelection(identity);
      if (cancelled) return;
      if (outcome?.kind === 'selected_status' && sameSelection(outcome.selection, identity)) {
        if (outcome.disposition.state === 'absent' || outcome.disposition.state === 'applied') {
          if (retained) await settleSelection(identity);
          if (cancelled) return;
          setNotice(outcome.disposition.state === 'applied'
            ? 'This exact document upload was applied. Other documents are unchanged.'
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
  const start = useCallback(async (inspect: boolean) => {
    if (dispatching.current || account.busy || account.running || !scope) return;
    dispatching.current = true;
    setNotice(null);
    try {
      const exact = pending ?? (!inspect && selected ? {
        scope, operationId: crypto.randomUUID(),
        receipt: { documentId: selected.documentId, sourceId: selected.sourceId, revision: selected.revision },
      } : null);
      if (!exact || !sameScope(exact.scope, scope)) return;
      // Persist before ANY dispatch. Selection change/unmount/error/cancel does
      // not clear it. Only a later exact readonly disposition may settle it.
      await retainSelection(exact);
      if (!mounted.current) return;
      if (!currentScope.current || !sameScope(exact.scope, currentScope.current)) {
        load();
        setNotice('The account changed before dispatch. This exact request was retained for its original account.');
        return;
      }
      setPending(exact);
      setError(null);
      await account.begin({ operation: inspect ? 'inspect_selected' : 'sync_selected', selection: exact });
    } catch (cause) { if (mounted.current) setError(cause instanceof Error ? cause.message : 'The upload identity could not be preserved.'); }
    finally { dispatching.current = false; }
  }, [account.begin, account.busy, account.running, scope, pending, selected, load]);
  return { pending, error, notice, otherScope, upload: () => start(false), inspect: () => start(true) };
}
