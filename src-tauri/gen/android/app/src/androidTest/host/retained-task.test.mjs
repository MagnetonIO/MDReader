import { test } from 'node:test';
import assert from 'node:assert/strict';
import { observeReceiverTask, sameRetainedReceiverTask } from './retained-task.mjs';

function dump({ token = 'b684fac', task = 49, state = 'RESUMED', finishing = false, root = true } = {}) {
  return `ACTIVITY MANAGER ACTIVITIES (dumpsys activity activities)
  * Task{d239795 #${task} type=standard A=10192:dev.mdreader.app}
    * Hist  #0: ActivityRecord{${token} u0 dev.mdreader.app/.MainActivity t${task}}
      rootOfTask=${root} task=Task{d239795 #${task} type=standard A=10192:dev.mdreader.app}
      packageName=dev.mdreader.app processName=dev.mdreader.app
      state=${state} stopped=${state === 'STOPPED'} delayedResume=false finishing=${finishing}
  mResumedActivity: ActivityRecord{aabbccd u0 public.launcher/.MainActivity t1}
`;
}

test('Back retains exact task and Activity, and warm launch resumes that same instance', () => {
  const before = observeReceiverTask(dump());
  assert.equal(sameRetainedReceiverTask(before, observeReceiverTask(dump({ state: 'STOPPED' })), 'STOPPED'), true);
  assert.equal(sameRetainedReceiverTask(before, observeReceiverTask(dump()), 'RESUMED'), true);
});

test('closed task and replacement Activity are refused even if host still sees the old PID', () => {
  const before = observeReceiverTask(dump());
  assert.equal(sameRetainedReceiverTask(before, observeReceiverTask('ACTIVITY MANAGER ACTIVITIES\n'), 'STOPPED'), false);
  assert.equal(sameRetainedReceiverTask(before, observeReceiverTask(dump({ task: 50 })), 'RESUMED'), false);
  assert.equal(sameRetainedReceiverTask(before, observeReceiverTask(dump({ token: 'd315be5' })), 'RESUMED'), false);
});

test('finishing, destroyed and merely pausing records cannot certify retained background', () => {
  const before = observeReceiverTask(dump());
  for (const change of [{ state: 'STOPPED', finishing: true }, { state: 'DESTROYED' }, { state: 'PAUSED' }, { state: 'STOPPED', root: false }]) {
    assert.equal(sameRetainedReceiverTask(before, observeReceiverTask(dump(change)), 'STOPPED'), false);
  }
});

test('receiver state ambiguity and oversized or malformed observations fail closed', () => {
  assert.throws(() => observeReceiverTask(dump() + dump({ token: 'd315be5' })), /More than one/);
  assert.throws(() => observeReceiverTask(dump().replace('state=RESUMED', 'state=RESUMED state=STOPPED')), /ambiguous/);
  assert.throws(() => observeReceiverTask(dump().replace('finishing=false', '')), /unavailable/);
  assert.throws(() => observeReceiverTask(dump().replace('t49}', 'unknown}')), /malformed/);
  assert.throws(() => observeReceiverTask('x'.repeat(4 * 1024 * 1024 + 1)), /bound/);
});

test('actual API34 public dump shape uses root task identity and both canonical component forms', () => {
  const value = dump({ token: '6b4f440', task: 51 });
  const short = observeReceiverTask(value);
  assert.equal(short.taskId, 51); assert.equal(short.activityToken, '6b4f440');
  assert.equal(short.rootOfTask, true);
  assert.deepEqual(observeReceiverTask(value.replace('dev.mdreader.app/.MainActivity', 'dev.mdreader.app/dev.mdreader.app.MainActivity')), short);
  assert.throws(() => observeReceiverTask(value.replace('rootOfTask=true task=Task{d239795 #51', 'rootOfTask=true task=Task{d239795 #52')), /ambiguous/);
});
