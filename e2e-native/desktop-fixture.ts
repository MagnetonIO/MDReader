/** Disposable native service owner and read-only witness for public corpus bytes.
 * No synthetic HTTP/provider adapter and no credentials or endpoint dumps. */
import { execFile, spawn, type ChildProcess } from 'node:child_process';
import { chmod, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { createConnection } from 'node:net';
import { tmpdir } from 'node:os';
import { join } from 'node:path';
import { promisify } from 'node:util';
import {
  createE2EProfile,
  assertE2EProfile,
  seedWindowsWorkspace,
  seedWorkspace,
  type E2EProfile,
} from './seed.ts';
import { E2E_BUNDLE_IDENTIFIER } from './flows.ts';
import { terminateOwnedProcess } from './lifecycle.ts';

export interface ExactReceipt { sourceId: string; path: string; revision: string }
export interface DesktopFixture {
  profile: E2EProfile;
  pid: number;
  readOriginalReceipt(path: string): Promise<ExactReceipt>;
  close(): Promise<void>;
}

async function request(profile: E2EProfile, pid: number, value: unknown): Promise<unknown> {
  assertE2EProfile(profile);
  const endpointBytes = await readFile(join(profile.dataDir, 'reader-v1', 'reader-endpoint.json'));
  if (endpointBytes.length > 4096) throw new Error('E2E endpoint exceeded its bound');
  const endpoint = JSON.parse(endpointBytes.toString('utf8')) as {
    protocol: number; pid: number; port: number; token: string;
  };
  if (endpoint.protocol !== 1 || endpoint.pid !== pid || !Number.isInteger(endpoint.port)
    || endpoint.port < 1 || endpoint.port > 65535 || typeof endpoint.token !== 'string'
    || !/^[a-f0-9]{64}$/.test(endpoint.token)) throw new Error('E2E service identity mismatch');
  return new Promise((resolve, reject) => {
    const socket = createConnection({ host: '127.0.0.1', port: endpoint.port });
    socket.setEncoding('utf8');
    let input = '';
    const fail = () => { socket.destroy(); reject(new Error('E2E native service reply unavailable')); };
    socket.setTimeout(15_000, fail);
    socket.once('error', fail);
    socket.once('connect', () => socket.write(JSON.stringify({ protocol: 1, token: endpoint.token, request: value }) + '\n'));
    socket.on('data', (bytes) => {
      input += bytes.toString('utf8');
      if (Buffer.byteLength(input) > 128 * 1024) { fail(); return; }
      if (!input.includes('\n')) return;
      socket.removeAllListeners('end'); socket.destroy();
      try {
        const reply = JSON.parse(input.slice(0, input.indexOf('\n'))) as { error: unknown; result: unknown };
        if (reply.error != null) { fail(); return; }
        resolve(reply.result);
      } catch { fail(); }
    });
    socket.once('end', fail);
  });
}

function receiptWitness(profile: E2EProfile, pid: number, corpus: string) {
  return async (path: string): Promise<ExactReceipt> => {
    if (!/^[a-z0-9/-]+\.md$/.test(path) || path.includes('..')) throw new Error('Invalid public fixture path');
    const sources = await request(profile, pid, { method: 'list_sources' }) as Array<{ id: string; enabled: boolean }>;
    const selected = sources.filter((source) => source.enabled);
    if (selected.length !== 1) throw new Error('Public corpus does not have one native source binding');
    const sourceId = selected[0]!.id;
    const read = await request(profile, pid, { method: 'read', params: { source_id: sourceId, path, revision: null } }) as {
      source_id: string; path: string; revision: string; markdown: string;
    };
    if (read.source_id !== sourceId || read.path !== path || !/^[a-f0-9]{64}$/.test(read.revision)
      || read.markdown !== await readFile(join(corpus, path), 'utf8')) throw new Error('Native receipt does not match exact public original');
    return { sourceId, path, revision: read.revision };
  };
}

/** WDIO workers are separate processes; transfer only root/PID identifiers,
 * then authenticate the existing exact owned service without restarting it. */
export function desktopWorkerWitness() {
  const root = process.env.MDREADER_E2E_PROFILE_ROOT;
  const pid = Number(process.env.MDREADER_E2E_SERVICE_PID);
  const corpus = process.env.MDREADER_E2E_CORPUS_ROOT;
  if (!root || !corpus || !Number.isSafeInteger(pid) || pid <= 0)
    throw new Error('Disposable native fixture is unavailable');
  const profile = createE2EProfile(E2E_BUNDLE_IDENTIFIER, root, root);
  return { readOriginalReceipt: receiptWitness(profile, pid, corpus) };
}

/** Bytes of service stderr kept for a startup failure message. */
const STDERR_TAIL_BYTES = 4096;

/** The exact current-user/SYSTEM-only descriptor the native owner creates
 * (`mdreader-capture` `Descriptor::private`). The Windows owner refuses any
 * profile object with an inherited or broader DACL and never repairs one, so
 * test-created objects must carry it explicitly. Directories pass, in order,
 * before the files inside them, so the files' explicit DACLs are applied last.
 * The script calls .NET directly, never a cmdlet: CI job steps run under pwsh 7,
 * whose PSModulePath leaks into this Windows PowerShell 5.1 child, and 5.1 then
 * cannot autoload Microsoft.PowerShell.Security (Get-Acl/Set-Acl). */
async function restrictWindowsPrivate(paths: string[]): Promise<void> {
  const script = [
    "$ErrorActionPreference = 'Stop'",
    '$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User.Value',
    "foreach ($path in $env:MDREADER_E2E_PRIVATE_PATHS -split \"`n\") {",
    '  if ([IO.Directory]::Exists($path)) {',
    '    $acl = [Security.AccessControl.DirectorySecurity]::new()',
    "    $acl.SetSecurityDescriptorSddlForm(\"O:${sid}D:P(A;OICI;FA;;;SY)(A;OICI;FA;;;$sid)\", 'Owner,Access')",
    '    [IO.Directory]::SetAccessControl($path, $acl)',
    '  } else {',
    '    $acl = [Security.AccessControl.FileSecurity]::new()',
    "    $acl.SetSecurityDescriptorSddlForm(\"O:${sid}D:P(A;;FA;;;SY)(A;;FA;;;$sid)\", 'Owner,Access')",
    '    [IO.File]::SetAccessControl($path, $acl)',
    '  }',
    '}',
  ].join('\n');
  // -EncodedCommand (UTF-16LE base64) sidesteps Windows command-line quoting.
  await promisify(execFile)(
    'powershell.exe',
    ['-NoProfile', '-NonInteractive', '-EncodedCommand', Buffer.from(script, 'utf16le').toString('base64')],
    { env: { ...process.env, MDREADER_E2E_PRIVATE_PATHS: paths.join('\n') }, windowsHide: true },
  );
}

export async function startDesktopFixture(binary: string, corpus: string): Promise<DesktopFixture> {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-e2e-'));
  await chmod(root, 0o700);
  const profile = createE2EProfile(E2E_BUNDLE_IDENTIFIER, root, root);
  let child: ChildProcess | undefined;
  let closed = false;
  const close = async () => {
    if (closed) return;
    await terminateOwnedProcess(child);
    await rm(root, { recursive: true, force: true });
    closed = true;
  };
  try {
    const marker = join(root, '.mdreader-e2e-profile-v1');
    await writeFile(marker, 'mdreader-native-e2e-profile-v1', { mode: 0o600 });
    if (process.platform === 'win32') {
      const journal = await seedWindowsWorkspace(profile, corpus);
      await restrictWindowsPrivate([root, marker, journal]);
    } else {
      await seedWorkspace(profile, corpus);
    }
    // The actual packaged service supports no-model mode. Text search still uses
    // the real SQLite index; semantic availability is never fabricated.
    child = spawn(binary, ['--mdreader-service', '--no-model', join(root, 'reader-v1')], {
      stdio: ['ignore', 'ignore', 'pipe'],
    });
    let stderr = Buffer.alloc(0);
    child.stderr?.on('data', (bytes: Buffer) => {
      stderr = Buffer.concat([stderr, bytes]);
      if (stderr.length > STDERR_TAIL_BYTES) stderr = stderr.subarray(stderr.length - STDERR_TAIL_BYTES);
    });
    const stderrTail = () => {
      const tail = stderr.toString('utf8').trim();
      return tail ? `; stderr: ${tail}` : '; no stderr';
    };
    let exit: string | undefined;
    child.once('error', (error) => { exit ??= `spawn error ${error.message}`; });
    child.once('exit', (code, signal) => { exit ??= signal ? `signal ${signal}` : `exit code ${code}`; });
    const pid = child.pid;
    if (!pid) throw new Error('E2E service could not start');
    const deadline = Date.now() + 15_000;
    for (;;) {
      if (exit) {
        // 'exit' can fire before the last stderr chunk is read.
        await new Promise((resolve) => setTimeout(resolve, 200));
        throw new Error(`E2E service exited before startup (${exit})${stderrTail()}`);
      }
      try { await request(profile, pid, { method: 'ping' }); break; }
      catch { if (Date.now() >= deadline) throw new Error(`E2E service startup timed out${stderrTail()}`); }
      await new Promise((resolve) => setTimeout(resolve, 100));
    }
    return {
      profile, pid, close,
      readOriginalReceipt: receiptWitness(profile, pid, corpus),
    };
  } catch (cause) { await close(); throw cause; }
}
