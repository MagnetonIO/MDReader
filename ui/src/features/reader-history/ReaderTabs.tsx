import { useTabsStore } from '../../stores/tabs';
import { useEditorStore } from '../../stores/editor';
import { isLocalDocument } from '../../stores/types';
import { TabStripView } from '../../shell/TabStripView';
import { useReaderHistoryStore } from './store';
export function ReaderTabs({ onLocalIntent, onCanonicalIntent, onRequestClose }: {
  onLocalIntent(): void;
  onCanonicalIntent(): void;
  onRequestClose(id: string): void;
}) {
  const legacy = useTabsStore();
  const editor = useEditorStore();
  const reader = useReaderHistoryStore();
  const oldTabs = legacy.tabs.map((tab) => ({
    id: tab.id, label: tab.docRef.name,
    title: isLocalDocument(tab.docRef) ? tab.docRef.path : tab.docRef.name,
    pinned: tab.pinned, dirty: editor.tabIsDirty(tab.id),
  }));
  const readerTabs = reader.data.tabs.map((tab) => ({
    id: `reader:${tab.id}`, label: tab.entries[tab.index]!.title,
    title: 'Exact read-only revision', pinned: tab.pinned, dirty: false,
  }));
  const tabs = [...oldTabs, ...readerTabs];
  return <TabStripView tabs={tabs}
    activeTabId={reader.data.activeTabId ? `reader:${reader.data.activeTabId}` : legacy.activeTabId}
    onSelect={(id) => {
      if (id.startsWith('reader:')) {
        onCanonicalIntent();
        reader.select(id.slice(7));
      } else {
        onLocalIntent();
        legacy.setActiveTab(id);
      }
    }}
    onClose={(id) => {
      if (id.startsWith('reader:')) reader.close(id.slice(7));
      else onRequestClose(id);
    }}
    onPin={(id) => id.startsWith('reader:') ? reader.pin(id.slice(7)) : legacy.togglePin(id)}
    onReorder={(from, to) => {
      if (from < oldTabs.length && to < oldTabs.length) legacy.reorderTabs(from, to);
      else if (from >= oldTabs.length && to >= oldTabs.length) {
        reader.reorder(from - oldTabs.length, to - oldTabs.length);
      }
    }} />;
}
