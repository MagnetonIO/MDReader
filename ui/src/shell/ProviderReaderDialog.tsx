/** Modal owner for explicit Settings source browsing; primary catalog reads inline. */
import { useEffect, useRef } from 'react';
import { CanonicalReader } from '../features/reader/CanonicalReader';
import type { ComponentProps } from 'react';

export function ProviderReaderDialog(props: ComponentProps<typeof CanonicalReader>) {
  const dialog = useRef<HTMLDialogElement>(null);
  useEffect(() => {
    const element = dialog.current;
    if (element?.showModal) element.showModal();
    else element?.setAttribute('open', '');
  }, []);
  return (
    <dialog
      ref={dialog}
      className="provider-reader-dialog"
      aria-label={props.sourceKind === 'local' ? 'Catalog document' : 'Cloud document'}
      onCancel={(event) => { event.preventDefault(); props.onClose(); }}
    >
      <CanonicalReader {...props} />
    </dialog>
  );
}
