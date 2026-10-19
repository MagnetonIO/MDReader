import { useEffect, useState } from 'react';
import { listLibrarySources } from '../platform/settings.js';

/**
 * Whether the active local root is a read-only Library member (Claude plans).
 * Until the snapshot answers it counts as writable; save_doc still refuses.
 */
export function useReadOnlyRoot(root: string | null, refreshKey: unknown): boolean {
  const [readOnlyRoot, setReadOnlyRoot] = useState<string | null>(null);
  useEffect(() => {
    if (!root) return;
    let live = true;
    listLibrarySources().then(snapshot => {
      if (!live) return;
      const source = snapshot.sources.find(candidate => candidate.path === root);
      setReadOnlyRoot(source && !source.writable ? root : null);
    }, () => {});
    return () => { live = false; };
  }, [root, refreshKey]);
  return root !== null && readOnlyRoot === root;
}
