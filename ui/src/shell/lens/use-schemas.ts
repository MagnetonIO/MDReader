/**
 * The registered-schema list, shared by the chrome that has to know what the
 * corpus can be read as: the reader toolbar's View As control and the lens band.
 *
 * Guarded like every other engine read in the shell — a workspace change
 * re-reads, and a superseded read is discarded rather than allowed to describe
 * a workspace the reader has already left. Failure is not an error state here:
 * before an index exists there is simply nothing to offer, and the chrome stays
 * quiet rather than raising an alarm about a feature the reader never invoked.
 */

import { useCallback, useEffect, useRef, useState } from 'react';

import type { SchemaSummary, SemanticsAdapter } from '../../platform/semantics.js';

export interface SchemaListing {
  schemas: SchemaSummary[];
  /** Re-read the list (after a ruling changes the counts, say). */
  reload: () => void;
}

export function useSchemas(
  adapter: SemanticsAdapter,
  workspaceKey: string | null,
  enabled: boolean,
  /** Advances when the active workspace's semantic engine can answer again. */
  ready: boolean,
): SchemaListing {
  const [schemas, setSchemas] = useState<SchemaSummary[]>([]);
  const [nonce, setNonce] = useState(0);
  const reqRef = useRef(0);

  useEffect(() => {
    if (!enabled || workspaceKey === null) {
      reqRef.current += 1;
      setSchemas([]);
      return;
    }
    const id = (reqRef.current += 1);
    adapter
      .listSchemas()
      .then((found) => {
        if (reqRef.current !== id) return;
        setSchemas(found);
      })
      .catch(() => {
        if (reqRef.current !== id) return;
        // No engine, or no index yet: nothing to view the corpus as. The
        // absence is the answer, so it is not reported as a failure.
        setSchemas([]);
      });
  }, [adapter, workspaceKey, enabled, ready, nonce]);

  const reload = useCallback(() => setNonce((value) => value + 1), []);
  return { schemas, reload };
}
