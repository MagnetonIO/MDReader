import type { PropsWithChildren } from 'react';
import type { AssetPreview } from '../../bindings';
import { AttachmentPreview } from './AttachmentPreview';

/** Keep the reader's layout and virtualizer measurements unchanged underneath
 * a linked-file preview. Visibility + inert hide it without collapsing height. */
export function AttachmentLayer({ preview, onClose, children }: PropsWithChildren<{
  preview: AssetPreview | null;
  onClose: () => void;
}>) {
  return <div className="attachment-layer">
    <div className="attachment-layer__origin" inert={Boolean(preview)}
      aria-hidden={preview ? true : undefined} style={{ visibility: preview ? 'hidden' : undefined }}>
      {children}
    </div>
    {preview ? <div className="attachment-layer__preview"><AttachmentPreview preview={preview} onClose={onClose} /></div> : null}
  </div>;
}
