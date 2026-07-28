import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { EventEmitter, once } from 'node:events';
import test from 'node:test';

import {
  resolveOwnedRunOutcome,
  terminateExclusiveExecutableOwner,
  waitForExclusiveExecutableOwner,
  waitForExclusiveProcessOwner,
  withOwnedProcesses,
} from './lifecycle.ts';

class ExitingProcess extends EventEmitter {
  exitCode: number | null = null;
  signalCode: NodeJS.Signals | null = null;
  killed = false;

  constructor(
    private readonly name: string,
    private readonly signals: string[],
  ) {
    super();
  }

  kill(signal: NodeJS.Signals = 'SIGTERM'): boolean {
    this.signals.push(`${this.name}:${signal}`);
    this.killed = true;
    queueMicrotask(() => {
      this.signalCode = signal;
      this.emit('exit', null, signal);
    });
    return true;
  }
}

test('teardown terminates only owned processes when adapter disposal fails', async () => {
  const signals: string[] = [];
  const app = new ExitingProcess('app', signals);
  const appium = new ExitingProcess('appium', signals);

  await assert.rejects(
    () => withOwnedProcesses(app, appium, false, async () => undefined, async () => { throw new Error('dispose failed'); }),
    /dispose failed/,
  );
  assert.deepEqual(signals, ['app:SIGTERM', 'appium:SIGTERM']);
});

test('teardown terminates owned processes when WebDriver construction fails', async () => {
  const signals: string[] = [];
  const app = new ExitingProcess('app', signals);
  const appium = new ExitingProcess('appium', signals);

  await assert.rejects(
    () => withOwnedProcesses(app, appium, false, async () => { throw new Error('remote failed'); }),
    /remote failed/,
  );
  assert.deepEqual(signals, ['app:SIGTERM', 'appium:SIGTERM']);
});

test('a delayed process-name handoff remains owned and is cleaned after preflight', async () => {
  const signals: string[] = [];
  const app = new ExitingProcess('app', signals);
  const observations = [[], [41]];

  await withOwnedProcesses(app, undefined, false, async () => {
    await waitForExclusiveProcessOwner(
      41,
      () => observations.shift() ?? [],
      async () => undefined,
    );
  });
  assert.deepEqual(signals, ['app:SIGTERM']);
});

test('a failed ownership check cannot leak the exact spawned child even with keep requested', async () => {
  const signals: string[] = [];
  const app = new ExitingProcess('app', signals);
  let sessionBound = false;

  await assert.rejects(
    () =>
      withOwnedProcesses(
        app,
        undefined,
        () => sessionBound,
        async () => {
          await waitForExclusiveProcessOwner(41, () => [99], async () => undefined);
          sessionBound = true;
        },
      ),
    /observed 99/,
  );
  assert.deepEqual(signals, ['app:SIGTERM']);
});

test('mac2 ownership waits for and returns the one exact executable', async () => {
  const observations = [
    [],
    [{ pid: 41, executable: '/reviewed/MDReader E2E.app/mdreader-e2e' }],
  ];
  const pid = await waitForExclusiveExecutableOwner(
    '/reviewed/MDReader E2E.app/mdreader-e2e',
    () => observations.shift() ?? [],
    async () => undefined,
  );
  assert.equal(pid, 41);
});

test('mac2 ownership rejects another worktree build with the same process name', async () => {
  await assert.rejects(
    () =>
      waitForExclusiveExecutableOwner(
        '/reviewed/MDReader E2E.app/mdreader-e2e',
        () => [
          {
            pid: 99,
            executable: '/other-worktree/MDReader E2E.app/mdreader-e2e',
          },
        ],
        async () => undefined,
      ),
    /other-worktree/,
  );
});

test('mac2 cleanup signals only the exact executable it observed', async () => {
  const observations = [
    [{ pid: 41, executable: '/reviewed/MDReader E2E.app/mdreader-e2e' }],
    [],
  ];
  const signals: string[] = [];
  await terminateExclusiveExecutableOwner(
    '/reviewed/MDReader E2E.app/mdreader-e2e',
    41,
    () => observations.shift() ?? [],
    (pid, signal) => signals.push(`${pid}:${signal}`),
    async () => undefined,
  );
  assert.deepEqual(signals, ['41:SIGTERM']);
});

test('mac2 cleanup refuses to signal a foreign same-name executable', async () => {
  const signals: string[] = [];
  await assert.rejects(
    () =>
      terminateExclusiveExecutableOwner(
        '/reviewed/MDReader E2E.app/mdreader-e2e',
        41,
        () => [
          {
            pid: 99,
            executable: '/other-worktree/MDReader E2E.app/mdreader-e2e',
          },
        ],
        (pid, signal) => signals.push(`${pid}:${signal}`),
        async () => undefined,
      ),
    /other-worktree/,
  );
  assert.deepEqual(signals, []);
});

test('mac2 cleanup never rebinds to a replacement pid at the same executable', async () => {
  const observations = [
    [{ pid: 41, executable: '/reviewed/MDReader E2E.app/mdreader-e2e' }],
    [{ pid: 42, executable: '/reviewed/MDReader E2E.app/mdreader-e2e' }],
  ];
  const signals: string[] = [];
  await assert.rejects(
    () =>
      terminateExclusiveExecutableOwner(
        '/reviewed/MDReader E2E.app/mdreader-e2e',
        41,
        () => observations.shift() ?? [],
        (pid, signal) => signals.push(`${pid}:${signal}`),
        async () => undefined,
        1,
        1,
      ),
    /42/,
  );
  assert.deepEqual(signals, ['41:SIGTERM']);
});

test('native cleanup failure never replaces the primary run failure', () => {
  const primary = new Error('primary flow failure');
  const cleanup = new Error('cleanup failure');
  assert.throws(
    () => resolveOwnedRunOutcome(primary, cleanup, { ok: false }),
    (error) => error === primary,
  );
  assert.throws(
    () => resolveOwnedRunOutcome(undefined, cleanup, { ok: false }),
    (error) => error === cleanup,
  );
});

test('teardown escalates only its owned child when SIGTERM is ignored', async () => {
  const child = spawn(
    process.execPath,
    [
      '-e',
      [
        "process.on('SIGTERM', () => {});",
        "process.send?.('ready');",
        'setInterval(() => {}, 1000);',
      ].join(''),
    ],
    { stdio: ['ignore', 'ignore', 'ignore', 'ipc'] },
  );
  const pid = child.pid;
  assert.ok(pid, 'controlled child has a pid');

  try {
    await once(child, 'message');
    await withOwnedProcesses(
      child,
      undefined,
      false,
      async () => undefined,
      undefined,
      { sigtermGraceMs: 25, sigkillGraceMs: 1_000 },
    );

    assert.equal(child.signalCode, 'SIGKILL');
    assert.throws(() => process.kill(pid, 0), { code: 'ESRCH' });
  } finally {
    if (child.exitCode === null && child.signalCode === null) {
      child.kill('SIGKILL');
      await once(child, 'exit');
    }
  }
});
