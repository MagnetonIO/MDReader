import type { ChildProcess } from 'node:child_process';

type ExitListener = (
  code: number | null,
  signal: NodeJS.Signals | null,
) => void;

type OwnedProcess = Pick<
  ChildProcess,
  | 'exitCode'
  | 'signalCode'
  | 'killed'
  | 'kill'
  | 'once'
  | 'removeListener'
> & { readonly pid?: number };

export interface TerminationOptions {
  readonly sigtermGraceMs?: number;
  readonly sigkillGraceMs?: number;
}

const DEFAULT_SIGTERM_GRACE_MS = 2_000;
const DEFAULT_SIGKILL_GRACE_MS = 2_000;

function hasExited(process: OwnedProcess): boolean {
  return process.exitCode !== null || process.signalCode !== null;
}

async function waitForExit(
  process: OwnedProcess,
  timeoutMs: number,
): Promise<boolean> {
  if (hasExited(process)) return true;

  return await new Promise<boolean>((resolve) => {
    let timer: NodeJS.Timeout | undefined;
    const settle = (exited: boolean) => {
      if (timer) clearTimeout(timer);
      process.removeListener('exit', onExit);
      resolve(exited);
    };
    const onExit: ExitListener = () => settle(true);

    process.once('exit', onExit);
    if (hasExited(process)) {
      settle(true);
      return;
    }
    timer = setTimeout(() => settle(false), timeoutMs);
  });
}

export async function terminateOwnedProcess(
  process: OwnedProcess | undefined,
  options: TerminationOptions = {},
): Promise<void> {
  if (!process || hasExited(process)) return;

  process.kill('SIGTERM');
  if (
    await waitForExit(
      process,
      options.sigtermGraceMs ?? DEFAULT_SIGTERM_GRACE_MS,
    )
  ) {
    return;
  }

  process.kill('SIGKILL');
  if (
    !(await waitForExit(
      process,
      options.sigkillGraceMs ?? DEFAULT_SIGKILL_GRACE_MS,
    ))
  ) {
    throw new Error(
      `owned E2E child ${process.pid ?? '<unknown>'} did not exit after SIGKILL`,
    );
  }
}

/**
 * Run work after the E2E app has launched, guaranteeing only this run's app and
 * Appium child are terminated on every failure path.
 */
export async function withOwnedProcesses<T>(
  app: OwnedProcess,
  appium: OwnedProcess | undefined,
  keep: boolean | (() => boolean),
  run: () => Promise<T>,
  dispose?: () => Promise<void>,
  terminationOptions: TerminationOptions = {},
): Promise<T> {
  try {
    return await run();
  } finally {
    try {
      await dispose?.();
    } finally {
      const keepApp = typeof keep === 'function' ? keep() : keep;
      await Promise.all([
        keepApp
          ? Promise.resolve()
          : terminateOwnedProcess(app, terminationOptions),
        terminateOwnedProcess(appium, terminationOptions),
      ]);
    }
  }
}

/** Wait for a just-spawned process to acquire its executable name exclusively. */
export async function waitForExclusiveProcessOwner(
  pid: number,
  observe: () => number[],
  pause: () => Promise<void>,
  tries = 20,
): Promise<void> {
  let owners: number[] = [];
  for (let attempt = 1; attempt <= tries; attempt++) {
    owners = observe();
    if (owners.length === 1 && owners[0] === pid) return;
    if (owners.length > 0) break;
    await pause();
  }
  throw new Error(
    `spawned pid ${pid} does not exclusively own the E2E process name; observed ` +
      (owners.join(', ') || 'none'),
  );
}
