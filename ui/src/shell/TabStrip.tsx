/** Legacy tab adapter; canonical receipts never enter its filesystem/editor model. */
import { useTabsStore } from '../stores/tabs';
import { useEditorStore } from '../stores/editor';
import { isLocalDocument } from '../stores/types';
import { TabStripView } from './TabStripView';
export interface TabStripProps {
  onLocalIntent(): void;
  onRequestClose(tabId: string): void;
}
export function TabStrip({ onLocalIntent, onRequestClose }: TabStripProps) {
  const state = useTabsStore();
  const editor = useEditorStore();
  return <TabStripView
    tabs={state.tabs.map((tab) => ({ id: tab.id, label: tab.docRef.name,
      title: isLocalDocument(tab.docRef) ? tab.docRef.path : tab.docRef.name,
      pinned: tab.pinned, dirty: editor.tabIsDirty(tab.id) }))}
    activeTabId={state.activeTabId}
    onSelect={(id) => { onLocalIntent(); state.setActiveTab(id); }}
    onPin={state.togglePin}
    onClose={(id) => { onLocalIntent(); onRequestClose(id); }}
    onReorder={state.reorderTabs}
  />;
}
