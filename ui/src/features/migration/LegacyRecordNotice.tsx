import './migration.css';
import type { CloudDocumentRef } from '../../stores/types';
import { documentKey } from '../../stores/types';
import { useEditorStore } from '../../stores/editor';

/** A retained session reference is metadata, never a renewed provider grant. */
export function LegacyRecordNotice({ reference, tabId, onMigration }: {
  reference: CloudDocumentRef;
  tabId: string;
  onMigration(): void;
}) {
  const buffer = useEditorStore((state) => state.entry(tabId, documentKey(reference)));
  function downloadBuffer() {
    if (!buffer) return;
    const url = URL.createObjectURL(new Blob([buffer.buffer], { type: 'text/markdown;charset=utf-8' }));
    const anchor = document.createElement('a');
    anchor.href = url;
    anchor.download = 'recovered-edit.md';
    anchor.click();
    window.setTimeout(() => URL.revokeObjectURL(url), 0);
  }
  return <section className="migration-record" aria-label="Retained historical document">
    <h1>{reference.name}</h1>
    <p>This saved reference belongs to the retired reader. Its session entry has been kept; opening it does not reconnect an account or fetch a different revision.</p>
    <button type="button" onClick={onMigration}>Migration &amp; archived records</button>
    {buffer ? <section aria-label="Retained unsaved edit">
      <h2>Retained editor buffer</h2>
      <p>This buffer has not been saved to the original. Download it before discarding the tab.</p>
      <textarea aria-label="Retained editor buffer" readOnly value={buffer.buffer} />
      <button type="button" onClick={downloadBuffer}>Download retained edit</button>
    </section> : null}
  </section>;
}
