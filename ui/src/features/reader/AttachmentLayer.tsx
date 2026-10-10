import { Component, createRef, type PropsWithChildren } from 'react';
import type { AssetPreview } from '../../bindings';
import { AttachmentPreview } from './AttachmentPreview';

/** Keep the reader's layout and virtualizer measurements unchanged underneath
 * a linked-file preview. Visibility + inert hide it without collapsing height. */
type Props = PropsWithChildren<{
  /** Selection identity, never reconstructed from a display path. */
  origin: unknown;
  preview: AssetPreview | null;
  onClose: () => void;
}>;

export class AttachmentLayer extends Component<Props> {
  private readonly retained = createRef<HTMLDivElement>();
  private returnFocus: HTMLElement | null = null;

  // Snapshot runs before DOM mutations: the link is still active and the
  // original is not inert yet. A layout effect would see Return's autoFocus.
  getSnapshotBeforeUpdate(previous: Props): HTMLElement | null {
    if (!previous.preview && this.props.preview && previous.origin === this.props.origin) {
      return document.activeElement instanceof HTMLElement && document.activeElement !== document.body
        ? document.activeElement : null;
    }
    return null;
  }

  componentDidUpdate(previous: Props, _state: unknown, focused: HTMLElement | null) {
    if (previous.origin !== this.props.origin) { this.returnFocus = null; return; }
    if (!previous.preview && this.props.preview) { this.returnFocus = focused; return; }
    if (previous.preview && !this.props.preview) {
      const target = this.returnFocus;
      this.returnFocus = null;
      const available = (element: HTMLElement | null) => Boolean(element?.isConnected
        && !element.closest('[inert], [hidden], [aria-hidden="true"]'));
      if (available(target)) target?.focus({ preventScroll: true });
      else if (available(this.retained.current)) this.retained.current?.focus({ preventScroll: true });
    }
  }

  render() {
    const { preview, onClose, children } = this.props;
    return <div className="attachment-layer">
      <div ref={this.retained} className="attachment-layer__origin" tabIndex={-1} inert={Boolean(preview)}
        aria-hidden={preview ? true : undefined} style={{ visibility: preview ? 'hidden' : undefined }}>
        {children}
      </div>
      {preview ? <div className="attachment-layer__preview"><AttachmentPreview preview={preview} onClose={onClose} /></div> : null}
    </div>;
  }
}
