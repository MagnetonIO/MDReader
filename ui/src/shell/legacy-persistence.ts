/** Existing ordinary persistence for hosts lacking the reviewed durable JSON
 * owner. This real plugin writer makes installed preservation unavailable;
 * autoSave:false does NOT prevent the plugin's exit-time saves. */
import { load, type Store } from '@tauri-apps/plugin-store';

const stores = new Map<'session' | 'settings', Promise<Store>>();
function store(kind: 'session' | 'settings'): Promise<Store> {
  let current = stores.get(kind);
  if (!current) { current = load(`${kind}.json`, { autoSave: false }); stores.set(kind, current); }
  return current;
}
export async function readLegacyPersistence(): Promise<{ session: unknown; settings: unknown }> {
  const [session, settings] = await Promise.all([store('session'), store('settings')]);
  return { session: await session.get('session'), settings: await settings.get('settings') };
}
export async function writeLegacyPersistence(kind: 'session' | 'settings', value: unknown): Promise<void> {
  const current = await store(kind);
  await current.set(kind, value);
  await current.save();
}
