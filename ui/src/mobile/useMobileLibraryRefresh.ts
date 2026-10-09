import { useCallback, useEffect, useRef } from 'react';
import { commands, type DeviceLibrary } from '../bindings';

/** A late list must not replace a more recently requested native snapshot. */
export function useMobileLibraryRefresh(publish: (library: DeviceLibrary) => void) {
  const epoch = useRef(0);
  const mounted = useRef(true);
  const apply = useRef(publish); apply.current = publish;
  useEffect(() => {
    mounted.current = true;
    return () => { mounted.current = false; epoch.current++; };
  }, []);
  return useCallback(async () => {
    const version = ++epoch.current;
    let reply: Awaited<ReturnType<typeof commands.mobileReaderList>>;
    try { reply = await commands.mobileReaderList(); }
    catch (cause) {
      if (mounted.current && version === epoch.current) throw cause;
      return;
    }
    if (!mounted.current || version !== epoch.current) return;
    if (reply.status === 'error') throw new Error(reply.error);
    apply.current(reply.data);
  }, []);
}
