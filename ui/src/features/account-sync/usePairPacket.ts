import { useEffect, useState } from 'react';
import type { AccountStatus } from './backend';
import { currentPairPacket } from './pairing-view';

type Packet = NonNullable<ReturnType<typeof currentPairPacket>>;
/** A public packet already returned by the owner remains shareable while a
 * harmless status/job update occurs. It is display evidence, never approval or
 * login authority; native code still validates every imported packet. */
export function usePairPacket(status: AccountStatus | null, kind: 'request' | 'approval'): Packet | null {
  const [retained, setRetained] = useState<{ owner: string; packet: Packet } | null>(null);
  const allowed = status?.configured && (kind === 'request'
    ? !status.connected && status.custody?.state === 'pair_candidate'
      && !status.custody.enrollmentExpired && !status.custody.materialStaged
    : status.connected && status.custody?.deviceApproval !== 'none');
  const owner = allowed && status ? `${status.activeLibraryId}:${status.selectedScope?.libraryId}:${status.selectedScope?.accountDigest}:${kind}` : null;
  const packet = status && allowed ? currentPairPacket(status, kind) : null;
  useEffect(() => {
    if (!owner) setRetained(null);
    else if (packet) setRetained({ owner, packet });
    else setRetained((previous) => previous?.owner === owner ? previous : null);
  }, [owner, packet?.text]);
  return packet ?? (owner && retained?.owner === owner ? retained.packet : null);
}
