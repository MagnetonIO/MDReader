import type { PublicPairPacket } from './public-packet';
export function PublicPacketIdentity({ packet }: { packet: PublicPairPacket }) {
  return <details className="settings-account-advanced"><summary>Compare full device identities</summary>
    <p>Compare every value with the other device using a trusted channel. A matching filename or device name is not enough.</p>
    <dl className="account-packet-identity"><dt>Account fingerprint</dt><dd>{packet.account}</dd><dt>Library</dt><dd>{packet.libraryId}</dd><dt>Device request</dt><dd>{packet.pairId}</dd>
      {packet.fingerprints.map(item => <div key={item.label}><dt>{item.label}</dt><dd>{item.value}</dd></div>)}
    </dl>
  </details>;
}
