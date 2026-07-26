import assert from 'node:assert/strict';
import test from 'node:test';

import { withOwnedProcesses } from './lifecycle.ts';

test('teardown terminates only owned processes when adapter disposal fails', async () => {
  const signals: string[] = [];
  const app = {
    exitCode: null,
    killed: false,
    kill: (signal: string) => {
      signals.push(`app:${signal}`);
      return true;
    },
  };
  const appium = {
    exitCode: null,
    killed: false,
    kill: (signal: string) => {
      signals.push(`appium:${signal}`);
      return true;
    },
  };

  await assert.rejects(
    () => withOwnedProcesses(app, appium, false, async () => undefined, async () => { throw new Error('dispose failed'); }),
    /dispose failed/,
  );
  assert.deepEqual(signals, ['app:SIGTERM', 'appium:SIGTERM']);
});

test('teardown terminates owned processes when WebDriver construction fails', async () => {
  const signals: string[] = [];
  const app = { exitCode: null, killed: false, kill: (signal: string) => (signals.push(`app:${signal}`), true) };
  const appium = { exitCode: null, killed: false, kill: (signal: string) => (signals.push(`appium:${signal}`), true) };

  await assert.rejects(
    () => withOwnedProcesses(app, appium, false, async () => { throw new Error('remote failed'); }),
    /remote failed/,
  );
  assert.deepEqual(signals, ['app:SIGTERM', 'appium:SIGTERM']);
});
