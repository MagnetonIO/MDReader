import { useEffect, useRef } from 'react';
import { MobileReadingAppearance } from './MobileReadingAppearance';
import type { Theme } from '../stores/settings';
import { AccountSyncView } from '../features/account-sync/AccountSyncPanel';
import type { useAccountSync } from '../features/account-sync/useAccountSync';
import type { DeviceReference } from '../features/device-reader/backend';

export function MobileSyncSettings({ account, selected, theme, onTheme, focusSync }: {
  account: ReturnType<typeof useAccountSync>; selected?: DeviceReference;
  theme: Theme; onTheme(theme: Theme): void; focusSync: number;
}) {
  const heading = useRef<HTMLHeadingElement>(null);
  useEffect(() => {
    if (!focusSync) return;
    heading.current?.focus();
    heading.current?.scrollIntoView?.({ block: 'start' });
  }, [focusSync]);
  return <section className="mobile-screen" aria-label="Settings">
    <h1>Settings</h1>
    <fieldset className="mobile-theme">
      <legend>Appearance</legend>
      {(['system', 'light', 'dark'] as const).map((choice) => <label key={choice}>
        <input type="radio" name="mobile-theme" checked={theme === choice} onChange={() => onTheme(choice)} />
        {choice[0]!.toUpperCase() + choice.slice(1)}
      </label>)}
    </fieldset>
    <MobileReadingAppearance />

    <div className="mobile-account-settings">
      <h2 ref={heading} tabIndex={-1}>Account &amp; sync</h2>
      {selected ? <p className="mobile-sync-selection">Selected for sync: <strong title={selected.title}>{selected.title || 'Untitled document'}</strong>. Upload starts only when you choose Sync this document.</p> : null}
      <AccountSyncView account={account} selectedDocument={selected} refreshOnOpen={false} />
    </div>
    <h2>Offline copies</h2>
    <p>Imports and pins are never automatically evicted. This device library allows 1,000 documents or 64 MiB.</p>
  </section>;
}
