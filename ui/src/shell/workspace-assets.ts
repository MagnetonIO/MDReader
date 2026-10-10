import { useEffect, useRef, useState } from 'react';
import type { RefObject } from 'react';
import type { AssetPreview, WorkspaceOriginReceipt } from '../bindings';
import { readWorkspaceAsset, readWorkspaceImage } from '../platform/workspace-assets';
import { useLocalImages } from '../features/reader/LocalSourceImage';

/** Exact saved-origin lifetime for the workspace reader and its split preview. */
export function useWorkspaceAssets(origin: WorkspaceOriginReceipt | undefined, action: RefObject<number>, onNotice: (notice: string | null) => void) {
  const selected = useRef(origin); selected.current = origin;
  const [attachment, setAttachment] = useState<{ origin: WorkspaceOriginReceipt; preview: AssetPreview } | null>(null);
  const images = useLocalImages(origin, readWorkspaceImage, Boolean(origin));
  useEffect(() => { setAttachment(null); }, [origin]);
  function close() { setAttachment(null); }
  function open(target: string) {
    if (!origin) { onNotice('Reload this document before opening its linked file.'); return; }
    const epoch = action.current;
    onNotice('Opening linked file…');
    void readWorkspaceAsset(origin, target, 'attachment').then((preview) => {
      if (action.current !== epoch || selected.current !== origin) return;
      setAttachment({ origin, preview }); onNotice(null);
    }).catch((cause: unknown) => {
      if (action.current === epoch && selected.current === origin) {
        onNotice(cause instanceof Error ? cause.message : 'The linked file could not be read.');
      }
    });
  }
  return { images, preview: attachment?.origin === origin ? attachment.preview : null, open, close };
}
