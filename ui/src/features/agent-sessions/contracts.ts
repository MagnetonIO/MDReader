/** Renderer projection of native agent activity: titles and Markdown paths only, never message text. */
export type AgentKind = 'claude' | 'codex';
export const AGENTS: readonly AgentKind[] = ['claude', 'codex'];
export const agentName = (agent: AgentKind) => agent === 'codex' ? 'Codex' : 'Claude Code';

export interface AgentActivitySession {
  id: string; agent: AgentKind; title: string; titleSource: string;
  cwd: string | null; gitBranch: string | null;
  startedAtMs: number; lastActivityMs: number; endedAtMs: number | null; fileCount: number;
}
/** An exact catalog document; absent when the file is not admitted (private, deleted or unreadable). */
export interface AgentActivityReference { sourceId: string; path: string; revision: string }
export interface AgentActivityFile {
  absPath: string; displayPath: string; op: 'create' | 'edit'; firstAtMs: number; lastAtMs: number;
  exists: boolean; isPlan: boolean; reference: AgentActivityReference | null; reason?: string | null;
}
export interface AgentActivityPage { sessions: AgentActivitySession[]; nextOffset: number | null }
export interface AgentActivityQuery { agent?: AgentKind; offset: number; limit: number; filter?: string }
export interface AgentTrackingStatus { enabled: boolean; available: boolean }
export type AgentTrackingState = Record<AgentKind, AgentTrackingStatus>;

export interface AgentActivityBackend {
  list(query: AgentActivityQuery): Promise<AgentActivityPage>;
  files(sessionId: string): Promise<AgentActivityFile[]>;
  rename(sessionId: string, title: string): Promise<void>;
  hide(sessionId: string): Promise<void>;
  /** Native resolves the session's recorded folder; absent when the platform cannot reveal it. */
  revealCwd?(sessionId: string): Promise<void>;
  trackingState(): Promise<AgentTrackingState>;
  setTracking(agent: AgentKind, enabled: boolean): Promise<AgentTrackingState>;
  loadOlder(days: number): Promise<void>;
  forget(agent: AgentKind): Promise<void>;
  /** Pushes `sessionId` whenever native records activity; there is no polling fallback. */
  onChanged(handler: (sessionId: string | null) => void): Promise<() => void>;
}
