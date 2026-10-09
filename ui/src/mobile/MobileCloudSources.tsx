import { useRef, useState } from 'react';
import { NativeProvidersPanel } from '../shell/NativeProvidersPanel';
import { CanonicalReader } from '../features/reader/CanonicalReader';
import { providerTarget } from '../shell/provider-reading';
import type { ProviderReaderReference } from '../platform/providers';

/** Native provider receipts stay provider receipts: no device copy or folder grant is inferred. */
export function MobileCloudSources({ onChanged, onReadingChanged }: { onChanged(): void; onReadingChanged(reading: boolean): void }) {
  const [reference, setReference] = useState<ProviderReaderReference | null>(null);
  const panel = useRef<HTMLDivElement>(null);
  const opener = useRef<HTMLElement | null>(null);
  function open(value: ProviderReaderReference) {
    const verified = providerTarget({ sourceId: value.sourceId, path: value.path,
      revision: value.revision, name: value.title ?? value.path, line: value.line ?? 1 });
    if (!verified) throw new Error('The exact provider receipt is unavailable.');
    opener.current = document.activeElement instanceof HTMLElement ? document.activeElement : null;
    setReference(verified);
    onReadingChanged(true);
  }
  function close() {
    setReference(null);
    onReadingChanged(false);
    requestAnimationFrame(() => {
      if (opener.current?.isConnected) opener.current.focus();
      else panel.current?.focus();
    });
  }
  return <div className="mobile-cloud-sources">
    <div ref={panel} hidden={!!reference} tabIndex={-1}>
      <NativeProvidersPanel initiallyExpanded allowEditing={false} onOpen={open} onSourcesChanged={onChanged} />
    </div>
    {reference ? <div className="mobile-cloud-reader">
      <CanonicalReader reference={reference} sourceKind="provider" documentActions={false} onClose={close} />
      <p className="mobile-cloud-reader__hint">Provider copies are read only. Image selection, sharing and export are available for imported device copies; this provider receipt does not grant those permissions.</p>
    </div> : null}
  </div>;
}
