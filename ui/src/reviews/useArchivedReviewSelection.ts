import { useLayoutEffect, useState } from 'react';
import type { CanonicalReaderReference } from '../features/reader/contracts';
import type { ProviderReaderReference } from '../platform/providers';
import type { ReviewTarget } from './ArchivedReviewsPanel';

type Selection = {
  target: ReviewTarget | null;
  returnTo: { kind: 'canonical'; origin: CanonicalReaderReference }
    | { kind: 'provider'; reference: ProviderReaderReference } | null;
};

/** Archive comparison never replaces the inline reader or restores an older selection. */
export function useArchivedReviewSelection(current: CanonicalReaderReference | null) {
  const [selection, setSelection] = useState<Selection | null>(null);
  const stale = selection?.returnTo?.kind === 'canonical' &&
    selection.returnTo.origin !== current;
  useLayoutEffect(() => {
    if (stale) setSelection(null);
  }, [stale]);
  return {
    // Hide before paint and unmount pending comparison work on a new selection.
    target: stale ? null : selection?.target ?? null,
    isOpen: !stale && selection !== null,
    openArchive() {
      setSelection({ target: null, returnTo: null });
    },
    openLocal(path: string, generation: number) {
      setSelection({ target: { kind: 'local', path, generation }, returnTo: null });
    },
    openCanonical(origin: CanonicalReaderReference, reference: ProviderReaderReference) {
      const { sourceId, path, revision } = reference;
      setSelection({ target: { kind: 'exact', reference: { sourceId, path, revision } }, returnTo: { kind: 'canonical', origin } });
    },
    openProvider(reference: ProviderReaderReference) {
      setSelection({ target: { kind: 'exact', reference }, returnTo: { kind: 'provider', reference } });
    },
    close() {
      const reference = selection?.returnTo?.kind === 'provider' ? selection.returnTo.reference : null;
      setSelection(null);
      return reference;
    },
  };
}
