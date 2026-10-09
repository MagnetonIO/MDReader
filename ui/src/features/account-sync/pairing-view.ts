import type { AccountStatus } from './backend';
import { parsePublicPacket, type PublicPairPacket } from './public-packet';

/** A completed local packet operation is neither enrollment nor a connection. */
export function currentPairPacket(status: AccountStatus, kind: 'request' | 'approval'): { text: string; packet: PublicPairPacket } | null {
  const outcome = status.latestJob?.state.state === 'completed' ? status.latestJob.state.outcome : null;
  if (outcome?.kind !== 'public_packet' || outcome.packetKind !== kind) return null;
  const library = status.activeLibraryId ?? status.selectedScope?.libraryId;
  if (!library) return null;
  try {
    const packet = parsePublicPacket(outcome.packet);
    if (packet.kind !== `mdreader-pair-${kind}` || packet.libraryId !== library
      || status.selectedScope && packet.account !== status.selectedScope.accountDigest) return null;
    return { text: outcome.packet, packet };
  } catch { return null; }
}
export function canReconcileEnrollment(status: AccountStatus): boolean {
  if (!status.retainedLibrary) return false;
  return status.custody?.state !== 'pair_candidate' && status.custody?.state !== 'recovery_candidate'
    || Boolean(status.custody?.materialStaged);
}

export function canRenewEnrollment(status: AccountStatus): boolean {
  return status.retainedLibrary && (status.custody?.state === 'bootstrap_pending'
    || status.custody?.state === 'recovery_candidate' && status.custody.materialStaged && status.custody.enrollmentExpired);
}
