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

export interface ExecutableProcessOwner {
  readonly pid: number;
  readonly executable: string;
}

type ObserveExecutableOwners = () => ExecutableProcessOwner[];
type SignalExecutableOwner = (pid: number, signal: NodeJS.Signals) => void;

function requireExactExecutableOwner(
  expectedExecutable: string,
  owners: ExecutableProcessOwner[],
): ExecutableProcessOwner | undefined {
  if (owners.length === 0) return undefined;
  if (
    owners.length === 1 &&
    owners[0]!.executable === expectedExecutable
  ) {
    return owners[0];
  }
  throw new Error(
    `expected one E2E process at ${expectedExecutable}; observed ` +
      owners
        .map((owner) => `${owner.pid}:${owner.executable}`)
        .join(', '),
  );
}

/**
 * Wait for mac2 to launch exactly the reviewed executable. mac2 owns the
 * application launch, so the PID is discovered only after the WebDriver
 * session exists; the executable path is the authority, not the shared process
 * name or bundle identifier.
 */
export async function waitForExclusiveExecutableOwner(
  expectedExecutable: string,
  observe: ObserveExecutableOwners,
  pause: () => Promise<void>,
  tries = 200,
): Promise<number> {
  let owners: ExecutableProcessOwner[] = [];
  for (let attempt = 1; attempt <= tries; attempt++) {
    owners = observe();
    const owner = requireExactExecutableOwner(expectedExecutable, owners);
    if (owner) return owner.pid;
    await pause();
  }
  throw new Error(
    `mac2 did not launch the reviewed E2E executable ${expectedExecutable}`,
  );
}

/**
 * Terminate only the one process whose loaded executable is the reviewed E2E
 * binary. This is the fallback when the WebDriver session cannot terminate its
 * own exact app path; a same-name process from another worktree is never
 * signalled.
 */
export async function terminateExclusiveExecutableOwner(
  expectedExecutable: string,
  observe: ObserveExecutableOwners,
  signal: SignalExecutableOwner,
  pause: () => Promise<void>,
  termTries = 20,
  killTries = 20,
): Promise<void> {
  let owner = requireExactExecutableOwner(expectedExecutable, observe());
  if (!owner) return;

  signal(owner.pid, 'SIGTERM');
  for (let attempt = 1; attempt <= termTries; attempt++) {
    await pause();
    owner = requireExactExecutableOwner(expectedExecutable, observe());
    if (!owner) return;
  }

  signal(owner.pid, 'SIGKILL');
  for (let attempt = 1; attempt <= killTries; attempt++) {
    await pause();
    owner = requireExactExecutableOwner(expectedExecutable, observe());
    if (!owner) return;
  }
  throw new Error(
    `reviewed E2E process ${owner.pid} did not exit after SIGKILL`,
  );
}
