import { spawn } from 'node:child_process';

const [apk, member, maximumText] = process.argv.slice(2);
const maximum = Number(maximumText);
if (!apk || !member || !Number.isSafeInteger(maximum) || maximum <= 0) {
  process.stderr.write('invalid Android native-library scanner arguments\n');
  process.exit(2);
}

const prefix = Buffer.from('MDREADER_E2E_BUILD_MANIFEST:');
const suffix = Buffer.from(':MDREADER_E2E_BUILD_MANIFEST_END');
const maximumManifestBytes = 16 * 1024;
let total = 0;
let pending = Buffer.alloc(0);
let capturing = false;
let manifest = null;
let failed = false;

function fail(message) {
  if (failed) return;
  failed = true;
  process.stderr.write(`${message}\n`);
  unzip.kill('SIGKILL');
}

function scan(bytes) {
  let input = Buffer.concat([pending, bytes]);
  pending = Buffer.alloc(0);
  while (input.length > 0 && !failed) {
    if (!capturing) {
      const start = input.indexOf(prefix);
      if (start < 0) {
        const retained = Math.min(input.length, prefix.length - 1);
        pending = input.subarray(input.length - retained);
        return;
      }
      if (manifest !== null) {
        fail('artifact has an ambiguous embedded E2E build manifest');
        return;
      }
      capturing = true;
      input = input.subarray(start);
    }

    const end = input.indexOf(suffix, prefix.length);
    if (end < 0) {
      if (input.length > maximumManifestBytes) {
        fail('artifact has an invalid embedded E2E build manifest');
        return;
      }
      pending = input;
      return;
    }
    const manifestEnd = end + suffix.length;
    manifest = Buffer.from(input.subarray(0, manifestEnd));
    capturing = false;
    input = input.subarray(manifestEnd);
  }
}

const unzip = spawn('unzip', ['-p', apk, member], {
  stdio: ['ignore', 'pipe', 'ignore'],
});

unzip.stdout.on('data', (chunk) => {
  total += chunk.length;
  if (total > maximum) {
    fail('Android native library stream exceeds the artifact safety bound');
    return;
  }
  scan(chunk);
});

unzip.on('error', () => fail('could not stream Android native library'));
unzip.on('close', (status) => {
  if (failed) {
    process.exitCode = 2;
    return;
  }
  if (status !== 0) {
    process.stderr.write('could not stream Android native library\n');
    process.exitCode = 2;
    return;
  }
  if (capturing) {
    process.stderr.write('artifact has an invalid embedded E2E build manifest\n');
    process.exitCode = 2;
    return;
  }
  if (manifest !== null) process.stdout.write(manifest);
});
