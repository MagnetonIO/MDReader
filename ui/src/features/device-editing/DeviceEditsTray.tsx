import { useEditorStore } from '../../stores/editor';
import { useDeviceEdits } from './store';

export function DeviceEditsTray({ onOpen }: { onOpen(key: string): void }) {
  const edits = useDeviceEdits((state) => state.edits);
  const entries = useEditorStore((state) => state.entries);
  const retained = Object.values(edits).filter((edit) => entries[`${edit.tabId} ${edit.key}`]?.dirty || edit.pending);
  if (!retained.length) return null;
  return <nav className="device-edit-drafts" aria-label="Markdown drafts">
    <h3>Markdown drafts</h3>
    {retained.map((edit) => <button type="button" key={edit.key} onClick={() => onOpen(edit.key)}>
      {edit.reference.title || 'Markdown copy'} · {edit.pending ? 'unconfirmed save' : 'unsaved'}
    </button>)}
  </nav>;
}
