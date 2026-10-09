import { useCallback, useEffect, useRef, useState } from 'react';
import type { useAccountSync } from '../useAccountSync';
import type { SelectedScope } from '../backend';
import { sameScope } from '../pending-selection';
import type { Choice, PendingEntry, PendingList, ResolutionChoice } from './model';
import { notifyDeviceReadChanged } from '../../device-reader/changed';

type Account = ReturnType<typeof useAccountSync>;

/** Observe actual service jobs; a failed/lost resolution never becomes a retry. */
export function usePendingChanges(account: Account, active: boolean) {
  const scope = account.status?.selectedScope;
  const [snapshot, setSnapshot] = useState<PendingList | null>(null);
  const [choice, setChoice] = useState<Choice | null>(null);
  const [attempt, setAttempt] = useState<Choice | null>(null);
  const [notice, setNotice] = useState<{ scope: SelectedScope; message: string } | null>(null);
  const seen = useRef<string | null>(null);
  const latest = account.status?.latestJob;
  const busy = account.busy || account.running || latest?.state.state === 'running';
  const connected = Boolean(account.status?.connected && scope);
  const entries = scope && snapshot && sameScope(scope, snapshot.scope) ? snapshot.entries : null;
  const currentChoice = scope && choice && sameScope(scope, choice.scope) ? choice : null;
  const currentAttempt = scope && attempt && sameScope(scope, attempt.scope) ? attempt : null;

  const refresh = useCallback(() => {
    if (!active || busy || !scope || !connected) return;
    setChoice(null);
    void account.begin({ operation: 'list_pending', scope });
  }, [account.begin, active, busy, scope, connected]);

  useEffect(() => {
    if (!active || !connected) { setSnapshot(null); setChoice(null); }
  }, [active, connected]);

  useEffect(() => {
    setSnapshot(null); setChoice(null);
  }, [scope?.accountDigest, scope?.libraryId, scope?.deviceId]);

  useEffect(() => {
    if (!scope || !latest || latest.state.state === 'running' || seen.current === latest.id) return;
    if (latest.state.state !== 'completed') {
      seen.current = latest.id;
      setSnapshot(null); setChoice(null);
      return;
    }
    const outcome = latest.state.outcome;
    if (outcome.kind === 'pending_list' && sameScope(outcome.scope, scope)) {
      seen.current = latest.id;
      setSnapshot(outcome);
      setAttempt(null);
      setChoice(null);
    } else if (outcome.kind === 'pending_resolved' && sameScope(outcome.scope, scope)) {
      seen.current = latest.id;
      setSnapshot(null); setChoice(null);
      if (attempt && sameScope(attempt.scope, scope) && attempt.entry.sequence === outcome.sequence
        && attempt.entry.revisionId === outcome.revisionId && attempt.choice === outcome.choice) {
        setAttempt(null);
        setNotice({ scope, message: outcome.choice === 'keep_local_state'
          ? 'Your pin and position are kept. The other device’s state is preserved in history; nothing was uploaded.'
          : 'Both copies are kept. The incoming copy is now in your Library.' });
      }
      if (outcome.document) notifyDeviceReadChanged(outcome.document);
    } else {
      seen.current = latest.id;
      setSnapshot(null); setChoice(null);
    }
  }, [latest, scope, attempt]);

  const review = (entry: PendingEntry, choice: ResolutionChoice) => {
    if (!scope || busy || currentAttempt || !entries?.includes(entry) || !entry.choices.includes(choice)
      || choice !== 'keep_both' && choice !== 'keep_local_state') return;
    setNotice(null);
    setChoice({ scope, entry, choice });
  };
  const confirm = () => {
    if (!currentChoice || busy || !connected || currentAttempt || !entries?.includes(currentChoice.entry)) return;
    const exact = currentChoice;
    setChoice(null); setAttempt(exact); setNotice(null);
    void account.begin({ operation: 'resolve_pending', scope: exact.scope,
      sequence: exact.entry.sequence, revisionId: exact.entry.revisionId,
      resolutionToken: exact.entry.resolutionToken, choice: exact.choice });
  };
  return { entries, busy, connected, choice: currentChoice, attempt: currentAttempt,
    notice: scope && notice?.scope && sameScope(scope, notice.scope) ? notice.message : null,
    refresh, review, confirm, cancel: () => setChoice(null) };
}
