/** Route tab shortcuts by owner; canonical receipts never reach legacy edit/file handlers. */
import { useTabsStore } from '../../stores/tabs';
import { useReaderHistoryStore } from './store';
export const readerActive = () => useReaderHistoryStore.getState().data.activeTabId !== null;
export function closeReaderTab(): boolean {
  const store = useReaderHistoryStore.getState(), id = store.data.activeTabId;
  if (!id) return false;
  store.close(id); return true;
}
export function stepReaderTab(direction: number): boolean {
  if (!readerActive()) return false;
  useReaderHistoryStore.getState().step(direction); return true;
}
export function createTabCycle(onLocalIntent: () => void, onCanonicalIntent: () => void) {
  let order: string[] | null = null;
  let index = 0;
  return {
    cycle(direction: number) {
      const legacy = useTabsStore.getState(), reader = useReaderHistoryStore.getState();
      const canonical = reader.data.mru.map((id) => `reader:${id}`);
      const local = legacy.mru.map((id) => `legacy:${id}`);
      if (!order) {
        order = reader.data.activeTabId ? [...canonical, ...local] : [...local, ...canonical];
        const active = reader.data.activeTabId ? `reader:${reader.data.activeTabId}` : `legacy:${legacy.activeTabId}`;
        index = Math.max(0, order.indexOf(active));
      }
      const available = new Set([...canonical, ...local]);
      order = order.filter((id) => available.has(id));
      if (!order.length) { order = null; return; }
      index = (index + direction + order.length) % order.length;
      const next = order[index]!;
      if (next.startsWith('reader:')) { onCanonicalIntent(); reader.select(next.slice(7)); }
      else { onLocalIntent(); legacy.setActiveTab(next.slice(7)); }
    },
    finish() { order = null; index = 0; },
  };
}
