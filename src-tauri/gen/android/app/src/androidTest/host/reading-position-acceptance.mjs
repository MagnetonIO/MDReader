#!/usr/bin/env node
/** Coordinator-only acceptance against the retained public Android profile.
 * Imports use a separate UID's normal OS grant. Only the shipped UI writes
 * reading positions; IPC inspection is read/search/list only. */
import { execFileSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { lstatSync, writeFileSync } from 'node:fs';
import { connectReceiver } from './webview-probe.mjs';
import { createStageRecorder } from './reading-position-stages.mjs';
import { readingPositionPackage } from './reading-position-package.mjs';
import { observeReceiverTask, sameRetainedReceiverTask } from './retained-task.mjs';
import { fileURLToPath } from 'node:url';

const flags = new Map();
const allowed = ['--serial', '--avd', '--run', '--output', '--opt-in-retained-synthetic-profile', '--resume-existing-reading-fixture', '--app-source-commit'];
for (let index = 2; index < process.argv.length; index += 2) {
  const key = process.argv[index], value = process.argv[index + 1];
  if (!allowed.includes(key) || !value || flags.has(key)) throw new Error('Invalid acceptance arguments.');
  flags.set(key, value);
}
const adbPath = '/opt/homebrew/share/android-commandlinetools/platform-tools/adb';
const serial = flags.get('--serial'), avd = flags.get('--avd'), run = flags.get('--run') ?? randomUUID();
const output = flags.get('--output'), app = 'dev.mdreader.app';
const known = { id: 'b5fd90eb-2c6a-430a-ab75-9476d50ed182', name: 'external-send.md',
  revision: '39627de4f71d547964423be57005a2a169e9f9e386bd54fcb9751113391962d0',
  source_id: '389e98d50972b3c06ff57570684509889e225b51db94c6975e2c7e70ab0cb6a4', bytes: 88,
  bodySha256: '4dae787d3cd36dc05a400a5168871e060b29900ead65a04c59299ec4d786a5a5' };
const resume = flags.get('--resume-existing-reading-fixture') === 'true';
if (flags.has('--resume-existing-reading-fixture') && !resume) throw new Error('Resume requires explicit true.');
const appPackage = readingPositionPackage(flags.get('--app-source-commit'), resume);
const appHash = appPackage.apkSha256;
const retainedRun = 'af50b48c-5a1e-420b-b4c8-5183a9f43ca4';
let acceptedLong = null;
const retainedLong = { id: '339c9e4a-26ce-4016-94fb-668e07e69d1e',
  source_id: '45881be69f22e8365e9cbb3f0a731644fdb8b87ff647ceddf3dc27a9b5e43d05',
  revision: 'b5c35ebe0212efba5e3ea87305edd8826dc77fc23b565d893d728e2b0fee5e5d' };
const name = 'external-reading-position.md', marker = `externalingress${run.replaceAll('-', '')}8`;
let body = `\uFEFF# External reading position acceptance\r\n\r\n${marker}\r\n\r\n`;
for (let section = 1; section <= 420; section++) {
  const number = String(section).padStart(3, '0');
  body += `## Section ${number}\r\n\r\nPosition checkpoint ${number}. ${marker}step${number}\r\n\r\n`;
}
const original = Buffer.from(body, 'utf8');
const sha = (value) => createHash('sha256').update(value).digest('hex');
const expected = { hash: sha(original), bytes: original.length };
const require = (value, message) => { if (!value) throw new Error(message); };
const pause = (milliseconds) => new Promise((resolve) => setTimeout(resolve, milliseconds));
function adb(args) {
  try { return execFileSync(adbPath, ['-s', serial, ...args], { encoding: 'utf8', timeout: 20_000, stdio: ['ignore', 'pipe', 'pipe'] }); }
  catch { throw new Error('Owned-emulator ADB operation failed.'); }
}
async function until(message, condition, timeout = 30_000) {
  const end = Date.now() + timeout;
  do { const value = await condition(); if (value) return value; await pause(200); } while (Date.now() < end);
  throw new Error(message);
}
function pid() {
  require(adb(['get-state']).trim() === 'device', 'The exact emulator is offline.');
  let result;
  try {
    result = execFileSync(adbPath, ['-s', serial, 'shell', 'pidof', app],
      { encoding: 'utf8', timeout: 20_000, stdio: ['ignore', 'pipe', 'pipe'] }).trim();
    require(/^[1-9]\d{0,8}$/.test(result), 'Receiver PID is invalid.');
  } catch (cause) {
    if (cause.status === 1 && cause.stdout === '' && cause.stderr === '') result = '';
    else throw new Error('Receiver process observation failed.');
  }
  require(adb(['get-state']).trim() === 'device', 'The emulator disconnected during PID observation.');
  return result;
}
function apkHash(packageName) {
  const paths = adb(['shell', 'pm', 'path', packageName]).trim().split('\n');
  const base = paths.find((line) => line.endsWith('/base.apk'))?.replace(/^package:/, '');
  require(base && /^\/data\/app\/[a-zA-Z0-9_./=+~-]+\/base\.apk$/.test(base), 'Installed APK path is invalid.');
  const hash = adb(['shell', 'sha256sum', base]).trim().split(/\s/)[0];
  require(/^[0-9a-f]{64}$/.test(hash), 'Installed APK hash is invalid.'); return hash;
}
const receiverActivities = ['dev.mdreader.app/.MainActivity', 'dev.mdreader.app/dev.mdreader.app.MainActivity'];
function foregroundActivities() {
  // A background WebView can still answer CDP. System input must belong to
  // the exact receiving Activity, with no other resumed display/Activity.
  const observations = adb(['shell', 'dumpsys', 'activity', 'activities']).split('\n')
    .filter((line) => /^\s*(?:mResumedActivity|topResumedActivity)\s*[:=]/.test(line));
  require(observations.length >= 1 && observations.length <= 8, 'Foreground Activity could not be confirmed.');
  return observations.map((line) => {
    const match = /ActivityRecord\{[^\r\n}]*\su\d+\s+([a-zA-Z0-9_.]+\/[a-zA-Z0-9_.$]+)\s/.exec(line);
    require(match, 'Foreground Activity observation is unavailable or ambiguous.'); return match[1];
  });
}
function receiverTask() {
  return observeReceiverTask(adb(['shell', 'dumpsys', 'activity', 'activities']));
}
function requireForegroundReceiver() {
  require(foregroundActivities().every((component) => receiverActivities.includes(component)),
    'The exact receiver is not the sole foreground Activity; no system input was sent.');
}
async function receiver() {
  const processId = await until('Receiver process did not appear.', pid);
  // Readiness may lag process startup. Retry only connecting this exact PID.
  const bridge = await until('Receiver WebView did not appear.', async () => {
    try { return await connectReceiver(adb, processId); } catch { return null; }
  });
  try {
    await until('Ordinary IPC did not become ready.', () => bridge.evaluate("typeof window.__TAURI_INTERNALS__?.invoke === 'function'"));
    return { pid: processId, bridge };
  } catch (cause) { bridge.close(); throw cause; }
}
async function sender(operation) {
  const operationId = randomUUID();
  adb(['shell', 'am', 'start', '-W', '-n', 'dev.mdreader.app.test/dev.mdreader.app.ExternalFixtureSenderActivity',
    '--ez', 'syntheticOwnedEmulator', 'true', '--ez', 'hostLaunch', 'true', '--es', 'hostOperationId', operationId,
    '--es', 'run', run, '--es', 'operation', operation]);
  const prefix = `run=${run} operation=${operation} operationId=${operationId} senderUid=`;
  const status = await until('Independent sender did not confirm the selected operation.', () =>
    adb(['logcat', '-d', '-v', 'brief', '-s', 'MDReaderSyntheticSender:I', '*:S']).split('\n')
      .find((line) => line.includes(prefix) && line.endsWith('result=ok')), 15_000);
  const match = /senderUid=(\d+) targetUid=(\d+) result=ok$/.exec(status);
  require(match && match[1] !== match[2], 'Independent sender UID was not proved.');
  return { senderUid: Number(match[1]), receiverUid: Number(match[2]) };
}
async function snapshot(bridge, allowLong, baseline) {
  const value = await bridge.evaluate(`(async () => {
    const invoke = window.__TAURI_INTERNALS__.invoke, library = await invoke('mobile_reader_list');
    if (library.pending.length) throw new Error('Unexpected pending imports; preserve this profile.');
    const known = ${JSON.stringify(known)};
    if (library.documents.some(doc => !(doc.id === known.id && doc.revision === known.revision && doc.name === known.name)
      && !(${JSON.stringify(allowLong)} && doc.name === ${JSON.stringify(name)}
        && (!${JSON.stringify(resume)} || (doc.id === ${JSON.stringify(retainedLong.id)}
          && doc.source_id === ${JSON.stringify(retainedLong.source_id)} && doc.revision === ${JSON.stringify(retainedLong.revision)}))))) throw new Error('Unknown documents; preserve this profile.');
    if (library.documents.filter(doc => doc.id === known.id).length !== 1 || library.documents.length > ${allowLong ? 2 : 1})
      throw new Error('Unexpected Library membership; preserve this profile.');
    const documents = [];
    for (const doc of library.documents) {
      const read = await invoke('mobile_reader_read', { id: doc.id, revision: doc.revision });
      for (const field of ['id','source_id','revision','name','bytes','pinned','line'])
        if (read.document[field] !== doc[field]) return null;
      const hash = Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(read.markdown))))
        .map(value => value.toString(16).padStart(2, '0')).join('');
      documents.push({ id: doc.id, source_id: doc.source_id, revision: doc.revision, name: doc.name, bytes: doc.bytes,
        pinned: doc.pinned, line: doc.line, bodySha256: hash });
    }
    const hits = ${allowLong ? `await invoke('mobile_reader_search', { query: ${JSON.stringify(marker)} })` : '[]'};
    return { documents, indexed: hits.filter(hit => hit.document.indexed).map(hit => ({id:hit.document.id,revision:hit.document.revision})) };
  })()`);
  if (!value) return null;
  const retained = value.documents.find((doc) => doc.id === known.id);
  require(retained.bodySha256 === known.bodySha256 && retained.bytes === known.bytes && retained.source_id === known.source_id,
    'The known exact original or receipt changed.');
  if (baseline) require(JSON.stringify(retained) === JSON.stringify(baseline), 'The existing synthetic document or reading state changed.');
  const added = value.documents.find((doc) => doc.name === name);
  if (added) {
    require(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(added.id) &&
      /^[0-9a-f]{64}$/.test(added.source_id) && /^[0-9a-f]{64}$/.test(added.revision) && added.bodySha256 === expected.hash &&
      added.bytes === expected.bytes && !added.pinned && Number.isSafeInteger(added.line) && added.line >= 1,
    'The new exact synthetic copy changed.');
  }
  if (added && acceptedLong) require(['id', 'source_id', 'revision'].every(field => added[field] === acceptedLong[field]),
    'The exact published long-document receipt changed.');
  return { retained, added, indexed: added && value.indexed.some((hit) => hit.id === added.id && hit.revision === added.revision) };
}
const viewExpression = `(() => {
  const reader = document.querySelector('[aria-label="Reader"]');
  if (!reader || reader.querySelector('.mobile-reader__bar strong')?.textContent !== ${JSON.stringify(name)}) return null;
  const scroll = reader.querySelector('.mdreader'); if (!scroll) return null;
  const rect = scroll.getBoundingClientRect(), blocks = Array.from(scroll.querySelectorAll('[data-block-index]'));
  let anchor = null;
  for (const block of blocks) {
    const relative = block.getBoundingClientRect().top - rect.top;
    if (relative <= 1 && (!anchor || relative > anchor.relative)) {
      const line = Number(block.querySelector('[data-line-start]')?.getAttribute('data-line-start'));
      anchor = { relative, line, block: Number(block.getAttribute('data-block-index')) };
    }
  }
  return { top: rect.top, height: rect.height, left: rect.left, width: rect.width, dpr: devicePixelRatio,
    innerWidth, scrollTop: scroll.scrollTop, scrollHeight: scroll.scrollHeight, mountedBlocks: blocks.length,
    virtualized: blocks.some(block => block.style.position === 'absolute'), anchorLine: anchor?.line ?? null, anchorBlock: anchor?.block ?? null, anchorRelative: anchor?.relative ?? null };
})()`;
async function view(bridge) { return bridge.evaluate(viewExpression); }
async function back(bridge) {
  require(await bridge.evaluate(`(() => { const reader = document.querySelector('[aria-label="Reader"]');
    if (reader?.querySelector('.mobile-reader__bar strong')?.textContent !== ${JSON.stringify(name)}) return false;
    const back = Array.from(reader.querySelectorAll('.mobile-reader__bar button')).find(node => node.textContent === 'Back');
    if (!back) return false; back.click(); return true; })()`), 'The shipped Reader Back control is unavailable.');
  await until('Back did not return to the shipped Library.', () => bridge.evaluate("!!document.querySelector('[aria-label=\"Library\"]') && !document.querySelector('[aria-label=\"Reader\"]')"));
}
async function open(bridge) {
  require(await bridge.evaluate(`(() => { const rows = Array.from(document.querySelectorAll('[aria-label="Library"] button'))
    .filter(node => node.getAttribute('aria-label') === ${JSON.stringify(name)});
    if (rows.length !== 1) return false; rows[0].click(); return true; })()`), 'The exact Library row is unavailable.');
  await until('The exact Reader did not render.', () => view(bridge));
}
async function restoredView(bridge, line) {
  return until('Reopening did not restore the scroll-derived source line.', async () => {
    const value = await view(bridge); return value?.anchorLine === line && value.scrollTop > 0 ? value : null;
  });
}

let current, stages, baseline = null, allowLong = false, savedLine = null;
let resultWritten = false;
async function observe() {
  const processId = pid();
  if (!current) return { receiverPid: processId ? Number(processId) : null, targetLine: savedLine, library: null, geometry: null, libraryRowLine: null };
  require(processId === current.pid, 'Receiver changed during metadata checkpoint.');
  const library = await until('Exact Library metadata did not settle.', () => snapshot(current.bridge, allowLong, baseline));
  const geometry = await view(current.bridge);
  const libraryRowLine = await current.bridge.evaluate(`(() => {
    const rows = Array.from(document.querySelectorAll('[aria-label="Library"] button'))
      .filter(node => node.getAttribute('aria-label') === ${JSON.stringify(name)});
    const match = rows.length === 1 ? /Line\\s+(\\d+)\\s*$/.exec(rows[0].querySelector('small')?.textContent ?? '') : null;
    return match ? Number(match[1]) : null;
  })()`);
  return { receiverPid: Number(processId), targetLine: savedLine, library, libraryRowLine,
    receiptSha256: sha(JSON.stringify(library)), geometry };
}
async function step(action, perform) { return stages.step(action, observe, perform); }
async function checkedBack() {
  return step('reader_back', async () => { requireForegroundReceiver(); await back(current.bridge); });
}
async function checkedOpen() {
  return step('library_open', async () => { requireForegroundReceiver(); await open(current.bridge); });
}
try {
  require(flags.get('--opt-in-retained-synthetic-profile') === 'true' && serial === 'emulator-5554' && avd === 'MDReaderReaderAcceptanceAPI34', 'Exact owned-emulator opt-in is required.');
  require(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(run) && output, 'Supply a UUIDv4 run and new metadata receipt.');
  require(!resume || run === retainedRun, 'Resume is restricted to the exact retained fixture run.');
  try { lstatSync(output); throw new Error('Receipt already exists.'); } catch (cause) { if (cause.code !== 'ENOENT') throw cause; }
  require(adb(['emu', 'avd', 'name']).split('\n')[0].trim() === avd && /ranchu|goldfish/.test(adb(['shell', 'getprop', 'ro.hardware']).trim()), 'Owned emulator identity mismatch.');
  require(apkHash(app) === appHash && pid(), 'The exact accepted shipping APK must already be running.');
  const testApkSha256 = apkHash('dev.mdreader.app.test');
  const size = /(?:Override|Physical) size: (\d+)x(\d+)/g;
  const sizes = [...adb(['shell', 'wm', 'size']).matchAll(size)];
  require(sizes.length, 'Android screen dimensions are unavailable.');
  const screenWidth = Number(sizes.at(-1)[1]), screenHeight = Number(sizes.at(-1)[2]);
  current = await receiver();
  allowLong = resume;
  const before = await until('The known synthetic Library did not settle.', () => snapshot(current.bridge, allowLong));
  baseline = before.retained;
  require(baseline.line === 1 && !baseline.pinned, 'The accepted baseline reading state changed.');
  require(!resume || (before.added && before.indexed), 'The exact two retained copies must already exist and be indexed.');
  const hostSourceCommit = execFileSync('git', ['rev-parse', 'HEAD'], { cwd: fileURLToPath(new URL('.', import.meta.url)), encoding: 'utf8' }).trim();
  stages = createStageRecorder(`${output}.stages.ndjson`, { fixtureRunId: run, mode: resume ? 'retained_two_resume' : 'fresh_import',
    hostSourceCommit, serial, avd, appSourceCommit: appPackage.sourceCommit, appApkSha256: appHash, testApkSha256 });
  await step('validated_preflight', async () => {});
  let uids = null, published;
  if (resume) {
    published = before;
    // No preparation, grant/import replay, reset, queue or position mutation.
    if (!(await view(current.bridge))) await checkedOpen();
  } else {
    uids = await step('prepare_external_original', () => sender('prepare-reading'));
    // The second known name is allowed after delivery begins; exact bytes and
    // receipt remain checked by every observation before subsequent UI input.
    allowLong = true;
    await step('deliver_external_view', async () => {
      require(JSON.stringify(await sender('reading')) === JSON.stringify(uids), 'Sender identity changed.');
    });
    published = await until('The long exact copy did not publish and index.', async () => {
      const value = await snapshot(current.bridge, true, baseline); return value?.added && value.indexed ? value : null;
    }, 60_000);
  }
  acceptedLong = { id: published.added.id, source_id: published.added.source_id, revision: published.added.revision };
  let scroll = await until('The imported long document did not open with mounted virtualized blocks.', async () => {
    const value = await view(current.bridge); return value?.virtualized && value.mountedBlocks > 0 ? value : null;
  });
  require(scroll.virtualized && scroll.mountedBlocks < 100 && scroll.scrollHeight > scroll.height * 20, 'The long document did not exercise real virtualization.');
  let swipes = 0;
  const initialAnchorLine = scroll.anchorLine;
  require(Number.isSafeInteger(initialAnchorLine) && initialAnchorLine >= 1, 'The initial rendered source line is unavailable.');
  const targetAdvanceLine = resume ? Math.max(published.added.line, initialAnchorLine) + 100 : 100;
  require(targetAdvanceLine <= 1500 && initialAnchorLine < targetAdvanceLine, 'The retained reader is too near the end for bounded gesture advancement.');
  while (!(scroll.anchorLine >= targetAdvanceLine) && swipes < 24) {
    require(Math.abs(scroll.innerWidth * scroll.dpr - screenWidth) <= 4 && scroll.height > 200, 'WebView-to-screen geometry is unavailable.');
    const x = Math.round((scroll.left + scroll.width / 2) * scroll.dpr);
    const from = Math.round((scroll.top + scroll.height * 0.78) * scroll.dpr), to = Math.round((scroll.top + scroll.height * 0.25) * scroll.dpr);
    require(x > 0 && x < screenWidth && to > 0 && from < screenHeight && from > to, 'Reader gesture would leave the owned viewport.');
    await step('android_swipe', async () => {
      requireForegroundReceiver();
      adb(['shell', 'input', 'swipe', String(x), String(from), String(x), String(to), '700']);
      swipes++; await pause(600);
    });
    scroll = await view(current.bridge);
    require(scroll?.virtualized, 'The exact Reader changed during scrolling.');
  }
  require(swipes >= 1 && scroll.anchorLine > initialAnchorLine && scroll.anchorLine >= targetAdvanceLine && scroll.anchorBlock > 40 && scroll.mountedBlocks < 100, 'Actual Android gestures did not advance the virtualized reader.');
  savedLine = scroll.anchorLine;
  await until('Scroll did not persist the exact source line.', async () => (await snapshot(current.bridge, true, baseline))?.added?.line === savedLine);
  await checkedBack();
  require((await snapshot(current.bridge, true, baseline)).added.line === savedLine, 'Reader close changed the saved line.');
  await checkedOpen();
  const reopenedView = await step('verify_first_reopen', () => restoredView(current.bridge, savedLine));
  await pause(600);
  require((await snapshot(current.bridge, true, baseline)).added.line === savedLine, 'Reopen replaced the saved position.');
  await checkedBack();
  const firstPid = current.pid;
  // Both launcher and external/component-entry root Back must retain the
  // actual Activity/task. A lingering PID with zero Activities is not warm.
  require(pid() === firstPid, 'Receiver identity changed before Android Back.');
  const beforeBackTask = receiverTask();
  require(beforeBackTask && beforeBackTask.rootOfTask && !beforeBackTask.finishing && beforeBackTask.state === 'RESUMED',
    'The exact receiving task is not retained and resumed before Android Back.');
  await step('android_back_background', async () => {
    requireForegroundReceiver();
    adb(['shell', 'input', 'keyevent', 'KEYCODE_BACK']);
    await until('Android Back did not background the retained receiver task.', () =>
      pid() === firstPid && sameRetainedReceiverTask(beforeBackTask, receiverTask(), 'STOPPED')
        && foregroundActivities().every((component) => !receiverActivities.includes(component)), 15_000);
  });
  const backgroundedAt = new Date().toISOString();
  require((await snapshot(current.bridge, true, baseline)).added.line === savedLine, 'Backgrounding changed the saved line.');
  await step('android_warm_launch', async () => {
    current.bridge.close(); current = null;
    adb(['shell', 'am', 'start', '-W', '-n', `${app}/dev.mdreader.app.MainActivity`]);
    current = await receiver(); require(current.pid === firstPid, 'Normal Back did not retain a warm receiver process.');
    await until('Warm launch replaced the retained Activity or task.', () =>
      sameRetainedReceiverTask(beforeBackTask, receiverTask(), 'RESUMED'));
    requireForegroundReceiver();
  });
  const warm = await snapshot(current.bridge, true, baseline);
  require(warm?.added?.line === savedLine && JSON.stringify({ ...warm.added, line: 1 }) === JSON.stringify({ ...published.added, line: 1 }), 'Warm resume changed the exact copy, receipt or saved line.');
  await checkedOpen(); const warmResumedView = await step('verify_warm_reopen', () => restoredView(current.bridge, savedLine));
  await pause(600); await checkedBack();
  const finalBeforeStop = await snapshot(current.bridge, true, baseline);
  require(finalBeforeStop?.added?.line === savedLine && finalBeforeStop.indexed &&
    JSON.stringify({ ...finalBeforeStop.added, line: 1 }) === JSON.stringify({ ...published.added, line: 1 }), 'Exact public Library was not verified before OS stop.');
  require(adb(['emu', 'avd', 'name']).split('\n')[0].trim() === avd && /ranchu|goldfish/.test(adb(['shell', 'getprop', 'ro.hardware']).trim()) &&
    apkHash(app) === appHash && apkHash('dev.mdreader.app.test') === testApkSha256 && pid() === current.pid,
  'Exact owned emulator/APK/process identity changed before OS stop.');
  requireForegroundReceiver();
  const oldPid = current.pid;
  // This explicit second gate is an OS force-stop of the guarded synthetic
  // Android profile, not a normal quit, signal to a desktop process or reset.
  await step('android_os_force_stop', async () => {
    require(pid() === oldPid && apkHash(app) === appHash && apkHash('dev.mdreader.app.test') === testApkSha256, 'Receiver identity changed before OS stop.');
    requireForegroundReceiver();
    current.bridge.close(); current = null;
    adb(['shell', 'am', 'force-stop', app]);
    await until('The exact receiver did not stop; cold relaunch was not attempted.', () => !pid(), 15_000);
  });
  const stoppedAt = new Date().toISOString();
  await step('android_cold_launch', async () => {
    adb(['shell', 'am', 'start', '-W', '-n', `${app}/dev.mdreader.app.MainActivity`]);
    current = await receiver(); require(current.pid !== oldPid, 'A new receiver PID is required.');
  });
  const after = await until('Retained documents did not reopen.', () => snapshot(current.bridge, true, baseline));
  require(after.added.line === savedLine && JSON.stringify({ ...after.added, line: 1 }) === JSON.stringify({ ...published.added, line: 1 }), 'Restart changed the exact copy, receipt or saved line.');
  await checkedOpen(); const restartedView = await step('verify_cold_reopen', () => restoredView(current.bridge, savedLine));
  await pause(600); await checkedBack();
  for (let count = 0; count < 4; count++) {
    await pause(250); const value = await snapshot(current.bridge, true, baseline);
    require(value?.added?.line === savedLine && value.indexed, 'Delayed restart changed position, indexing or receipts.');
  }
  const inspectedUids = await step('inspect_external_original', () => sender('inspect-reading'));
  require(!uids || JSON.stringify(inspectedUids) === JSON.stringify(uids), 'External original preservation could not be confirmed.');
  uids = inspectedUids;
  require(apkHash(app) === appHash && apkHash('dev.mdreader.app.test') === testApkSha256, 'APK identity changed during acceptance.');
  await stages.finish('success', observe);
  const receipt = { kind: 'android-scroll-reading-position', result: 'success', mode: resume ? 'retained_two_resume' : 'fresh_import',
    checkpoints: `${output}.stages.ndjson`, lastCheckpoint: stages.last, fixtureRunId: run, avd, serial, appSourceCommit: appPackage.sourceCommit, appApkSha256: appHash, testApkSha256, ...uids,
    existingDocument: baseline, document: after.added, exactOriginalSha256: expected.hash, originalBytes: expected.bytes,
    fixedSections: 420, swipeCount: swipes, savedLine, scrollView: scroll, reopenedView, warmResumedView, restartedView,
    normalBack: 'Exact Activity/task backgrounded and resumed; same process retained',
    retainedTaskId: beforeBackTask.taskId, retainedActivityToken: beforeBackTask.activityToken,
    backgroundedAt, warmReceiverPid: Number(firstPid),
    processStop: 'Android am force-stop on the exact owned synthetic profile', stoppedAt,
    oldReceiverPid: Number(oldPid), restartedReceiverPid: Number(current.pid),
    evidence: [resume ? 'retained_known_external_os_import' : 'external_uid_os_grant', 'exact_bom_crlf_copy', 'indexed_native_search', 'android_swipe_scroll', 'virtualized_reader',
      'ui_close_reopen_saved_line', 'normal_back_background_warm_resume', 'os_force_stop_cold_relaunch',
      'stable_receipt_no_duplicate', 'external_original_preserved', 'existing_document_preserved'],
    limits: ['exact_reviewed_source_and_apk_pair_only', 'debug_webview_read_only_inspection_and_ui_controls', 'owned_api34_emulator_only', 'no_physical_device_or_new_release_claim'],
    cleanup: 'External fixed fixture and both imported copies retained; no reset or deletion.' };
  writeFileSync(output, JSON.stringify(receipt, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
  resultWritten = true;
  console.log('Android scroll-derived reading position checks passed. Metadata receipt saved.');
} catch (cause) {
  const failure = cause instanceof Error ? cause.message : 'Reading position acceptance could not be confirmed.';
  if (stages && !resultWritten) {
    const final = await stages.finish('failure', observe, failure);
    writeFileSync(output, JSON.stringify({ kind: 'android-scroll-reading-position', result: 'failure', fixtureRunId: run,
      mode: resume ? 'retained_two_resume' : 'fresh_import', serial, avd, appSourceCommit: appPackage.sourceCommit, appApkSha256: appHash, savedLine,
      checkpoints: `${output}.stages.ndjson`, finalCheckpoint: final,
      cleanup: 'Both known public copies and the exact external fixture retained; no reset/reimport/deletion.' }, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
  }
  console.error(failure);
  process.exitCode = 1;
} finally { current?.bridge.close(); stages?.close(); }
