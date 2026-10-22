import { catalogReference } from '../catalog/backend';
import type { CanonicalReaderReference } from '../reader/contracts';
import type { AgentActivityFile, AgentActivitySession } from './contracts';

export const LIVE_WINDOW_MS = 5 * 60_000;

/** Live means native has not seen the session end and it wrote recently. */
export function isLive(session: AgentActivitySession, now: number): boolean {
  return session.endedAtMs === null && now - session.lastActivityMs < LIVE_WINDOW_MS;
}

export function relativeTime(at: number, now: number): string {
  const minutes = Math.max(0, Math.floor((now - at) / 60_000));
  if (minutes < 1) return 'now';
  if (minutes < 60) return `${minutes}m`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d`;
  return new Date(at).toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
}

export function sessionTooltip(session: AgentActivitySession): string {
  return [session.cwd, session.gitBranch ? `Branch ${session.gitBranch}` : null,
    `Started ${new Date(session.startedAtMs).toLocaleString()}`].filter(Boolean).join('\n');
}

export const fileName = (file: AgentActivityFile) => file.displayPath.split('/').pop() || file.displayPath;

/** Throws when native handed over a reference the catalog contract rejects. */
export function fileReference(file: AgentActivityFile): CanonicalReaderReference | null {
  if (!file.reference || !file.exists) return null;
  const { sourceId, path, revision } = file.reference;
  return catalogReference({ kind: 'exact', sourceId, path, revision }, fileName(file));
}
