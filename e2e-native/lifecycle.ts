import type { ChildProcess } from 'node:child_process';

type OwnedProcess = Pick<ChildProcess, 'exitCode' | 'killed' | 'kill'>;

function terminate(process: OwnedProcess | undefined): void {
  if (process?.exitCode === null && !process.killed) process.kill('SIGTERM');
}

/**
 * Run work after the E2E app has launched, guaranteeing only this run's app and
 * Appium child are terminated on every failure path.
 */
export async function withOwnedProcesses<T>(
  app: OwnedProcess,
  appium: OwnedProcess | undefined,
  keep: boolean,
  run: () => Promise<T>,
  dispose?: () => Promise<void>,
): Promise<T> {
  try {
    return await run();
  } finally {
    try {
      await dispose?.();
    } finally {
      if (!keep) terminate(app);
      terminate(appium);
    }
  }
}
