import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import { EventEmitter, once } from 'node:events';
import test from 'node:test';

import { waitForExclusiveProcessOwner, withOwnedProcesses } from './lifecycle.ts';

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
