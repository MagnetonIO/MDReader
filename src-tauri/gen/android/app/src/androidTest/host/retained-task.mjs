/** Bounded public ActivityManager observations. Never reads app files/IPC. */
const receiverComponents = new Set([
  'dev.mdreader.app/.MainActivity',
  'dev.mdreader.app/dev.mdreader.app.MainActivity',
]);

export function observeReceiverTask(dump) {
  if (typeof dump !== 'string' || Buffer.byteLength(dump, 'utf8') > 4 * 1024 * 1024) {
    throw new Error('Receiver task observation exceeds its bound.');
  }
  const lines = dump.split('\n'), records = [];
  for (let index = 0; index < lines.length; index++) {
    const line = lines[index];
    if (!/^\s*\* Hist\s+#\d+:/.test(line)) continue;
    const header = /^(\s*)\* Hist\s+#\d+:\s+ActivityRecord\{([a-fA-F0-9]+)\s+u\d+\s+([a-zA-Z0-9_.]+\/[a-zA-Z0-9_.$]+)\s+t([1-9]\d{0,8})(?:\s[^}]*)?\}/.exec(line);
    if (!header) {
      if (line.includes('dev.mdreader.app/')) throw new Error('Receiver task observation is malformed.');
      continue;
    }
    if (!receiverComponents.has(header[3])) continue;
    const indent = header[1].length, block = [];
    for (let next = index + 1; next < lines.length; next++) {
      const candidate = lines[next];
      if (candidate.trim() && /^\s*/.exec(candidate)[0].length <= indent) break;
      block.push(candidate);
    }
    const details = block.join('\n');
    const states = Array.from(details.matchAll(/\bstate=([A-Z_]+)\b/g));
    const finishing = Array.from(details.matchAll(/\bfinishing=(true|false)\b/g));
    const roots = Array.from(details.matchAll(/\brootOfTask=(true|false)\s+task=Task\{[a-fA-F0-9]+\s+#([1-9]\d{0,8})\s/g));
    if (states.length !== 1 || finishing.length !== 1 || roots.length !== 1 || roots[0][2] !== header[4]) throw new Error('Receiver task state is unavailable or ambiguous.');
    records.push({ taskId: Number(header[4]), activityToken: header[2], rootOfTask: roots[0][1] === 'true', state: states[0][1], finishing: finishing[0][1] === 'true' });
  }
  if (records.length > 1) throw new Error('More than one receiving Activity is retained.');
  return records[0] ?? null;
}

export function sameRetainedReceiverTask(before, after, state) {
  return before !== null && after !== null && before.rootOfTask && after.rootOfTask && !before.finishing && !after.finishing
    && before.taskId === after.taskId && before.activityToken === after.activityToken
    && after.state === state;
}
