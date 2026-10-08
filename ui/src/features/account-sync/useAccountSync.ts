import { useCallback, useEffect, useRef, useState } from 'react';
import { accountBackend, accountError, type AccountAction, type AccountBackend, type AccountStatus, type AccountRequest } from './backend.js';

/** Polling observes the same service actor. Closing Settings never cancels its job. */
export function useAccountSync(backend: AccountBackend = accountBackend, observing = true) {
  const [status, setStatus] = useState<AccountStatus | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [jobId, setJobId] = useState<string | null>(null);
  const epoch = useRef(0);
  const mounted = useRef(true);
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; epoch.current++; };
  }, []);
  useEffect(() => {
    if (!observing) { epoch.current++; setBusy(false); }
  }, [observing]);
  const current = (version: number) => mounted.current && epoch.current === version;

  const refresh = useCallback(async () => {
    const version = ++epoch.current;
    setBusy(true);
    setError(null);
    try {
      const reply = await backend.request({ operation: 'status' });
      if (!current(version)) return;
      if (reply.operation !== 'status') throw new Error('invalid reply');
      setStatus(reply.status);
      setJobId(reply.status.latestJob?.state.state === 'running' ? reply.status.latestJob.id : null);
    } catch (cause) {
      if (current(version)) setError(accountError(cause));
    } finally { if (current(version)) setBusy(false); }
  }, [backend]);

  const begin = useCallback(async (request: Extract<AccountRequest, { operation: 'begin' | 'sync_selected' | 'inspect_selected' }>) => {
    const version = ++epoch.current;
    setBusy(true);
    setError(null);
    try {
      const reply = await backend.request(request);
      if (!current(version)) return;
      if (reply.operation !== 'begun') throw new Error('invalid reply');
      setJobId(reply.jobId);
    } catch (cause) {
      if (current(version)) setError(accountError(cause));
    } finally { if (current(version)) setBusy(false); }
  }, [backend]);

  const act = useCallback((action: AccountAction) => begin({ operation: 'begin', intent: action }), [begin]);

  const stop = useCallback(async (disconnect: boolean) => {
    const exact = jobId ?? (status?.latestJob?.state.state === 'running' ? status.latestJob.id : null);
    if (!disconnect && !exact) return;
    const version = ++epoch.current;
    setBusy(true);
    setError(null);
    try {
      const reply = await backend.request(disconnect ? { operation: 'disconnect' } : { operation: 'cancel', jobId: exact! });
      if (!current(version)) return;
      if (reply.operation !== 'complete') throw new Error('invalid reply');
      setJobId(null);
      await refresh();
    } catch (cause) {
      if (current(version)) setError(accountError(cause));
    } finally { if (current(version)) setBusy(false); }
  }, [backend, jobId, status, refresh]);

  useEffect(() => {
    if (!jobId || busy || !observing) return;
    let cancelled = false;
    const started = Date.now();
    let timer: ReturnType<typeof setTimeout>;
    const poll = async () => {
      const version = epoch.current;
      try {
        const reply = await backend.request({ operation: 'poll', jobId });
        if (cancelled || !current(version)) return;
        if (reply.operation !== 'job' || !reply.job || reply.job.id !== jobId) {
          setError('This job is no longer retained. Refresh account state before retrying.');
          setJobId(null);
          return;
        }
        setStatus((previous) => previous ? { ...previous, latestJob: reply.job } : previous);
        if (reply.job.state.state !== 'running') {
          setJobId(null);
          await refresh();
          return;
        }
        if (Date.now() - started >= 12 * 60_000) {
          setError('Status polling paused. Refresh to inspect the service job; it was not automatically cancelled.');
          setJobId(null);
          return;
        }
        timer = setTimeout(() => { void poll(); }, 1000);
      } catch (cause) {
        if (!cancelled && current(version)) { setError(accountError(cause)); setJobId(null); }
      }
    };
    void poll();
    return () => { cancelled = true; clearTimeout(timer); };
  }, [backend, jobId, busy, observing, refresh]);

  return { status, error, busy, running: jobId !== null, refresh, act, begin, cancel: () => stop(false), disconnect: () => stop(true) };
}
