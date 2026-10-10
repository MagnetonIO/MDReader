/** Metadata-only, synchronous durable checkpoints. No fixture bodies or CDP
 * expressions enter this ledger. A pending action is recorded before input. */
import { closeSync, fsyncSync, openSync, writeSync } from 'node:fs';

export function createStageRecorder(path, identity) {
  const fd = openSync(path, 'wx', 0o600);
  let bytes = 0, records = 0, action = null, last = null;
  const append = (value) => {
    const record = { sequence: ++records, at: new Date().toISOString(), ...value };
    const data = Buffer.from(JSON.stringify(record) + '\n');
    if (records > 256 || bytes + data.length > 1024 * 1024) throw new Error('Metadata checkpoint bound exhausted; preserve the profile.');
    let offset = 0;
    while (offset < data.length) {
      const written = writeSync(fd, data, offset, data.length - offset);
      if (written <= 0) throw new Error('Metadata checkpoint could not be written.');
      offset += written;
    }
    bytes += data.length; fsyncSync(fd); last = record;
    return record;
  };
  append({ phase: 'identity', ...identity });
  return {
    async step(name, observe, perform) {
      if (action) throw new Error('Overlapping acceptance actions are forbidden.');
      action = name;
      append({ phase: 'before', action, metadata: await observe() });
      const value = await perform();
      append({ phase: 'success', action, metadata: await observe() });
      action = null;
      return value;
    },
    async finish(result, observe, failure = null) {
      let metadata = null, observation = 'available';
      try { metadata = await observe(); } catch { observation = 'unavailable'; }
      return append({ phase: 'result', result, action, failure, observation, metadata });
    },
    get last() { return last; },
    close() { closeSync(fd); },
  };
}
