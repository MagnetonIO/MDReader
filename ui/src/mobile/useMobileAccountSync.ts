import { useEffect, useRef, useState } from 'react';
import { useAccountSync } from '../features/account-sync/useAccountSync';
import { DEVICE_READ_CHANGED } from '../features/device-reader/changed';

/** One native actor observer survives screen changes. Foreground/reconnect
 * observations do not sign in, enqueue uploads or replay uncertain operations. */
export function useMobileAccountSync(screen: string, refreshLibrary: () => Promise<void>) {
  const [foreground, setForeground] = useState(() => document.visibilityState !== 'hidden');
  const account = useAccountSync(undefined, foreground);
  const refresh = useRef(refreshLibrary); refresh.current = refreshLibrary;
  const observed = useRef<string | null>(null);
  useEffect(() => {
    if (foreground) void account.refresh();
  }, [foreground, screen, account.refresh]);
  useEffect(() => {
    const visible = () => {
      const shown = document.visibilityState !== 'hidden';
      setForeground(shown);
      if (shown) void refresh.current();
    };
    const returned = () => {
      if (document.visibilityState === 'hidden') return;
      void account.refresh();
      void refresh.current();
    };
    const changed = () => { if (document.visibilityState !== 'hidden') void refresh.current(); };
    document.addEventListener('visibilitychange', visible);
    window.addEventListener('focus', returned);
    window.addEventListener('online', returned);
    window.addEventListener(DEVICE_READ_CHANGED, changed);
    return () => {
      document.removeEventListener('visibilitychange', visible);
      window.removeEventListener('focus', returned);
      window.removeEventListener('online', returned);
      window.removeEventListener(DEVICE_READ_CHANGED, changed);
    };
  }, [account.refresh]);
  const job = account.status?.latestJob;
  useEffect(() => {
    if (!foreground || !job || job.id === observed.current || job.state.state !== 'completed') return;
    const outcome = job.state.outcome;
    if (outcome.kind !== 'sync' && outcome.kind !== 'pending_resolved'
      && !(outcome.kind === 'selected_status' && outcome.disposition.state === 'applied')) return;
    observed.current = job.id;
    void refresh.current();
  }, [foreground, job]);
  return account;
}
