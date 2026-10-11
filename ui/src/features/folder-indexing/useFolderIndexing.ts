import { useEffect, useMemo, useRef, useState } from 'react';
import { folderIndexingBackend, type FolderIndexStatus, type FolderIndexingBackend } from '../../platform/folder-indexing';

/** Read native all-folder status, independently from the currently viewed workspace. */
export function useFolderIndexing(active: boolean, sourceKey: string, backend: FolderIndexingBackend = folderIndexingBackend) {
  const identity = useMemo(() => ({}), [active, sourceKey, backend]);
  const latest = useRef(identity); latest.current = identity;
  const serial = useRef(0), timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [value, setValue] = useState<{ identity: object; rows: FolderIndexStatus[] } | null>(null);
  const [action, setAction] = useState<{ identity: object; busy: string | null; error: string | null } | null>(null);
  function cancelPoll() { if (timer.current !== null) clearTimeout(timer.current); timer.current = null; }
  async function load(task: () => Promise<FolderIndexStatus[]>, busy: string | null = null) {
    cancelPoll(); const generation = ++serial.current, target = identity;
    setAction({ identity: target, busy, error: null });
    try {
      const rows = await task();
      if (latest.current !== target || generation !== serial.current) return;
      setValue({ identity: target, rows }); setAction({ identity: target, busy: null, error: null });
      if (active && rows.some(row => row.monitorEnabled && ['discovering', 'indexing'].includes(row.phase)))
        timer.current = setTimeout(() => { void load(() => backend.state()); }, 2000);
    } catch (cause) {
      if (latest.current === target && generation === serial.current)
        setAction({ identity: target, busy: null, error: cause instanceof Error ? cause.message : 'Folder indexing status is unavailable.' });
    }
  }
  useEffect(() => {
    if (active) void load(() => backend.state());
    return () => { serial.current += 1; cancelPoll(); };
  }, [identity]);
  return {
    rows: value?.identity === identity ? value.rows : null,
    busy: action?.identity === identity ? action.busy : null,
    error: action?.identity === identity ? action.error : null,
    refresh: () => load(() => backend.state(), 'refresh'),
    setMonitoring: (sourceId: string, enabled: boolean) => load(() => backend.setMonitoring(sourceId, enabled), sourceId),
  };
}
