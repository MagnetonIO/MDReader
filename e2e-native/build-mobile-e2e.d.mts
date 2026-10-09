/** Runtime contracts of the standalone JavaScript mobile wrapper. */
export function mobileBuildArgs(platform: string): string[];
export interface MobileBuildOperations {
  stage?: (sourceCommit: string) => () => void;
  run?: (args: string[], sourceCommit: string) => number;
  reportCleanupError?: (error: unknown) => void;
}
export function runMobileBuild(
  platform: string, sourceCommit?: string, operations?: MobileBuildOperations,
): number;
