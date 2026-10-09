/** Runtime contracts of the standalone JavaScript build wrapper. */
export function resolveSourceCommit(configured?: string, readHead?: () => string): string;
export function pnpmInvocation(
  args: string[], platform?: string, commandProcessor?: string,
): { command: string; args: string[] };
export function stageMobileSourceCommit(configured: string, destination?: string): () => void;
export function runPnpm(args: string[], sourceCommit: string): number;
