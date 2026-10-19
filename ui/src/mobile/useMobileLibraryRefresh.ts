import { useCallback, useEffect, useRef } from 'react';
import type { DeviceLibrary } from '../bindings';
import { useReaderBackend, type ReaderBackend } from '../platform/backend';

/** A late list must not replace a more recently requested native snapshot. */
export function useMobileLibraryRefresh(publish: (library: DeviceLibrary) => void) {
  const backend = useReaderBackend();
  const epoch = useRef(0);
  const mounted = useRef(true);
  const apply = useRef(publish); apply.current = publish;
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; epoch.current++; };
  }, []);
  return useCallback(async () => {
    const version = ++epoch.current;
    let reply: Awaited<ReturnType<ReaderBackend['mobile']['mobileReaderList']>>;
    try { reply = await backend.mobile.mobileReaderList(); }
    catch (cause) {
      if (mounted.current && version === epoch.current) throw cause;
      return;
    }
    if (!mounted.current || version !== epoch.current) return;
    if (reply.status === 'error') throw new Error(reply.error);
    apply.current(reply.data);
  }, [backend]);
}
