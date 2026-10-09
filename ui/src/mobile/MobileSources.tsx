import { useState } from 'react';
import type { AccountStatus } from '../features/account-sync/backend';
import { DeviceAssetStorage } from './DeviceAssetStorage';
import { MobileCloudSources } from './MobileCloudSources';

export function MobileSources({ status, onSync, onAssetsChanged, onSourcesChanged }: {
  status: AccountStatus | null; onSync(): void; onAssetsChanged(): void; onSourcesChanged(): void;
}) {
  const [reading, setReading] = useState(false);
  return <section className="mobile-screen" aria-label="Sources">
    <div hidden={reading}>
    <h1>Sources</h1>
    <article className="mobile-source">
      <h2>Imported Markdown</h2>
      <p>Original files and reading state are durable on this device. Imports do not grant access to device folders.</p>
    </article>
    <article className="mobile-source mobile-source--sync">
      <h2>Encrypted device sync</h2>
      <p role="status">{!status ? 'Sync status has not been confirmed.'
        : !status.configured ? 'Encrypted sync is unavailable in this build.'
        : status.connected ? 'Connected. Choose which documents and reading state to sync.'
        : status.retainedLibrary ? 'Your sync library is retained. Sign in to reconnect.'
        : 'Connect this device to your MDReader sync library.'}</p>
      <button type="button" className="mobile-touch" onClick={onSync}>
        {status?.connected ? 'Manage sync' : status?.configured ? 'Set up sync' : 'View sync settings'}
      </button>
    </article>
    <DeviceAssetStorage onChanged={onAssetsChanged} />
    </div>
    <MobileCloudSources onChanged={onSourcesChanged} onReadingChanged={setReading} />
  </section>;
}
