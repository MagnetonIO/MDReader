#!/usr/bin/env node
/** Coordinator-run, opt-in owned-emulator acceptance. No receiver instrumentation,
 * production debug commands, direct queue calls, imports or data resets. */
import { execFileSync } from 'node:child_process';
import { createHash, randomUUID } from 'node:crypto';
import { writeFileSync } from 'node:fs';
import { connectReceiver } from './webview-probe.mjs';

const flags = new Map();
for (let index = 2; index < process.argv.length; index += 2) flags.set(process.argv[index], process.argv[index + 1]);
const serial = flags.get('--serial'), avd = flags.get('--avd'), action = flags.get('--action');
const run = flags.get('--run') ?? randomUUID();
const output = flags.get('--output');
const app = 'dev.mdreader.app';
const senderComponent = 'dev.mdreader.app.test/dev.mdreader.app.ExternalFixtureSenderActivity';
const sha = (value) => createHash('sha256').update(value).digest('hex');
function require(value, message) { if (!value) throw new Error(message); }
function adb(args, absent = false) {
  try { return execFileSync('adb', ['-s', serial, ...args], { encoding: 'utf8', timeout: 20_000, stdio: ['ignore', 'pipe', 'pipe'] }); }
  catch (cause) { if (absent && cause.status === 1) return ''; throw new Error('Owned-emulator ADB operation failed.'); }
}
async function until(message, condition, timeout = 60_000) {
  const end = Date.now() + timeout;
  do { try { const result = await condition(); if (result) return result; } catch {} await new Promise((resolve) => setTimeout(resolve, 200)); } while (Date.now() < end);
  throw new Error(message);
}
const pid = () => adb(['shell', 'pidof', app], true).trim();
function apkHash(packageName) {
  const paths = adb(['shell', 'pm', 'path', packageName]).trim().split('\n');
  const base = paths.find((line) => line.endsWith('/base.apk'))?.replace(/^package:/, '');
  require(base && /^\/data\/app\/[a-zA-Z0-9_./=+~-]+\/base\.apk$/.test(base), 'Installed APK identity is unavailable.');
  const value = adb(['shell', 'sha256sum', base]).trim().split(/\s/)[0];
  require(/^[0-9a-f]{64}$/.test(value), 'Installed APK hash is unavailable.'); return value;
}
async function stop() {
  adb(['shell', 'am', 'force-stop', app]);
  await until('Receiver did not stop; cold delivery was not attempted.', () => !pid(), 10_000);
}
async function receiver() {
  const currentPid = await until('Receiver PID did not appear.', pid);
  const bridge = await until('Receiver debug WebView did not become available.', () => connectReceiver(adb, currentPid));
  try {
    await until('Ordinary receiver IPC did not become ready.', () => bridge.evaluate("typeof window.__TAURI_INTERNALS__?.invoke === 'function'"));
    return { pid: currentPid, bridge };
  } catch (cause) { bridge.close(); throw cause; }
}
async function send(operation) {
  const operationId = randomUUID();
  adb(['shell', 'am', 'start', '-W', '-n', senderComponent, '--ez', 'syntheticOwnedEmulator', 'true',
    '--ez', 'hostLaunch', 'true', '--es', 'hostOperationId', operationId, '--es', 'run', run, '--es', 'operation', operation]);
  const prefix = `run=${run} operation=${operation} operationId=${operationId} senderUid=`;
  const status = await until('Independent synthetic sender did not confirm the selected operation.', () =>
    adb(['logcat', '-d', '-v', 'brief', '-s', 'MDReaderSyntheticSender:I', '*:S']).split('\n').find((line) => line.includes(prefix) && line.endsWith('result=ok')), 15_000);
  const uids = /senderUid=(\d+) targetUid=(\d+) result=ok$/.exec(status);
  require(uids && uids[1] !== uids[2], 'Sender and receiver UIDs must differ.');
  return { senderUid: Number(uids[1]), receiverUid: Number(uids[2]) };
}
function snapshotExpression(name, marker) {
  return `(async () => {
    const invoke = window.__TAURI_INTERNALS__.invoke;
    const library = await invoke('mobile_reader_list');
    if (library.pending.length || library.documents.length !== 1 || library.documents[0].name !== ${JSON.stringify(name)}) return null;
    const doc = library.documents[0];
    const read = await invoke('mobile_reader_read', { id: doc.id, revision: doc.revision });
    for (const field of ['id','source_id','revision','name','bytes','pinned']) if (read.document[field] !== doc[field]) return null;
    const hash = Array.from(new Uint8Array(await crypto.subtle.digest('SHA-256', new TextEncoder().encode(read.markdown))))
      .map(value => value.toString(16).padStart(2, '0')).join('');
    const hits = await invoke('mobile_reader_search', { query: ${JSON.stringify(marker)} });
    if (!hits.some(hit => hit.document.id === doc.id && hit.document.source_id === doc.source_id && hit.document.revision === doc.revision && hit.document.indexed)) return null;
    return { id: doc.id, source_id: doc.source_id, revision: doc.revision, name: doc.name, bytes: doc.bytes, pinned: doc.pinned, bodySha256: hash };
  })()`;
}
async function verify(bridge, name, marker, expected) {
  const value = await until('Exact cold-import copy or real indexed search did not become available.', () => bridge.evaluate(snapshotExpression(name, marker)));
  require(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(value.id) &&
    /^[0-9a-f]{64}$/.test(value.source_id) && /^[0-9a-f]{64}$/.test(value.revision), 'Native receipt identity was invalid.');
  require(value.bodySha256 === expected.hash && value.bytes === expected.bytes, 'Exact synthetic BOM/CRLF copy changed.');
  return value;
}

try {
  require(flags.get('--opt-in-owned-empty-emulator') === 'true', 'Explicit owned-empty-emulator opt-in is required.');
  require(/^emulator-\d+$/.test(serial ?? '') && /^mdreader[a-zA-Z0-9_-]*$/i.test(avd ?? ''), 'Use only an explicitly named MDReader-owned emulator.');
  require(['VIEW', 'SEND'].includes(action) && output, 'Specify VIEW or SEND and a new receipt output file.');
  require(/^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/.test(run), 'Fixture run ID must be a canonical UUIDv4.');
  require(adb(['emu', 'avd', 'name']).split('\n')[0].trim() === avd, 'The emulator AVD identity did not match.');
  const hardware = adb(['shell', 'getprop', 'ro.hardware']).trim();
  require(/ranchu|goldfish/.test(hardware), 'Physical devices are refused.');
  const appApkSha256 = apkHash(app), testApkSha256 = apkHash('dev.mdreader.app.test');

  // Baseline initializes no fixture. It only verifies an empty synthetic Library;
  // the subsequent force-stop is mandatory before real cold selected delivery.
  adb(['shell', 'am', 'start', '-W', '-n', `${app}/dev.mdreader.app.MainActivity`]);
  const baseline = await receiver();
  try {
    const empty = await baseline.bridge.evaluate("window.__TAURI_INTERNALS__.invoke('mobile_reader_list').then(value => ({ documents: value.documents.length, pending: value.pending.length }))");
    require(empty.documents === 0 && empty.pending === 0, 'Existing Library is preserved. Use a fresh owned emulator; no selection was sent.');
  } finally { baseline.bridge.close(); }
  await stop();
  const uids = await send('prepare'); // Sender preparation must not start receiver.
  require(!pid(), 'Receiver was running immediately before cold selected delivery.');
  const before = new Date().toISOString();
  const deliveredUids = await send(action === 'VIEW' ? 'view' : 'send');
  require(JSON.stringify(uids) === JSON.stringify(deliveredUids), 'Sender/receiver UID identity changed.');
  const cold = await receiver();
  const name = action === 'VIEW' ? 'external-view.md' : 'external-send.md';
  const marker = `externalingress${run.replaceAll('-', '')}${action === 'VIEW' ? 0 : 1}`;
  const original = Buffer.from(`\uFEFF# External synthetic acceptance\r\n\r\n${marker}\r\n`, 'utf8');
  const expected = { hash: sha(original), bytes: original.length };
  let first;
  try {
    first = await verify(cold.bridge, name, marker, expected);
    if (action === 'SEND') {
      // Use the shipped Library row to open; never invoke a queue/import command.
      await until('The cold share did not appear in the actual Library.', () => cold.bridge.evaluate(`(() => {
        const row = Array.from(document.querySelectorAll('[aria-label="Library"] button')).find(node => node.getAttribute('aria-label') === ${JSON.stringify(name)});
        if (!row) return false; row.click(); return true;
      })()`));
    }
    await until('Cold selected Markdown did not render in the actual Reader.', () => cold.bridge.evaluate(`document.querySelector('[aria-label="Reader"]')?.textContent.includes(${JSON.stringify(marker)}) ?? false`));
    await send('inspect'); // SDK sender rechecks every exact synthetic original.
  } finally { cold.bridge.close(); }
  const receiptHash = sha(JSON.stringify(first));
  await stop();
  const restartAt = new Date().toISOString();
  adb(['shell', 'am', 'start', '-W', '-n', `${app}/dev.mdreader.app.MainActivity`]);
  const reopened = await receiver();
  try {
    require(reopened.pid !== cold.pid, 'A distinct receiver PID is required after the recorded OS stop.');
    require(sha(JSON.stringify(await verify(reopened.bridge, name, marker, expected))) === receiptHash, 'Restart changed exact copies or replayed the selection.');
    for (let count = 0; count < 5; count++) {
      await new Promise((resolve) => setTimeout(resolve, 200));
      require(sha(JSON.stringify(await verify(reopened.bridge, name, marker, expected))) === receiptHash, 'Delayed restart replay changed exact receipts.');
    }
    await send('inspect');
  } finally { reopened.bridge.close(); }
  const receipt = { kind: 'android-cold-selected-intent', action, fixtureRunId: run, avd, hardware, appApkSha256, testApkSha256, ...uids,
    baselineEmpty: true, receiverPidBeforeDelivery: null, deliveredAt: before, coldReceiverPid: Number(cold.pid),
    receiverPidBeforeRestart: null, restartedAt: restartAt, restartedReceiverPid: Number(reopened.pid),
    exactOriginalSha256: expected.hash, stableReceiptHash: receiptHash, document: first,
    evidence: ['external_uid_os_intent', 'actual_reader_dom', 'exact_native_read', 'indexed_native_search', 'original_preservation', 'os_process_restart_no_duplicate'],
    limits: ['debug_webview', 'owned_emulator_only', 'no_physical_device_or_share_sheet_visual_claim'] };
  writeFileSync(output, JSON.stringify(receipt, null, 2) + '\n', { flag: 'wx', mode: 0o600 });
  console.log(`Cold ${action} and restart checks passed. Metadata receipt saved.`);
} catch (cause) {
  console.error(cause instanceof Error ? cause.message : 'Cold acceptance could not be confirmed.');
  process.exitCode = 1;
}
