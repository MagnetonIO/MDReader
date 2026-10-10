import assert from 'node:assert/strict';
import { execFileSync, spawn, spawnSync } from 'node:child_process';
import { chmodSync, copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, unlinkSync, writeFileSync } from 'node:fs';
import { tmpdir } from 'node:os';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';
import { test } from 'node:test';
import { runMobileBuild } from './build-mobile.mjs';
import { stageHandoff } from './release-handoff.mjs';

const HERE = dirname(fileURLToPath(import.meta.url));
const COMMIT = '0123456789abcdef0123456789abcdef01234567';
const RUNTIME_FLAGS = ['CARGO_NET_OFFLINE', 'ORT_SKIP_DOWNLOAD', 'ORT_OFFLINE'];
const pause = (ms) => new Promise((done) => setTimeout(done, ms));
function fixture() {
  const temporary = mkdtempSync(join(tmpdir(), 'mdreader-mobile-source-'));
  const root = join(temporary, 'repo');
  const bin = join(temporary, 'bin');
  mkdirSync(join(root, 'scripts/development'), { recursive: true });
  mkdirSync(join(root, 'src-tauri'));
  mkdirSync(bin);
  for (const file of ['build-mobile.mjs', 'release-handoff.mjs']) copyFileSync(join(HERE, file), join(root, 'scripts/development', file));
  writeFileSync(join(root, '.gitignore'), '/src-tauri/.release-source-commit\n');
  writeFileSync(join(root, 'src-tauri/generated.plist'), 'original\n');
  const isolated = { ...process.env, GIT_CONFIG_NOSYSTEM: '1', GIT_CONFIG_GLOBAL: '/dev/null' };
  const git = (...args) => execFileSync('git', args, { cwd: root, env: isolated, encoding: 'utf8', stdio: ['ignore', 'pipe', 'pipe'] });
  git('init');
  git('add', '.');
  git('-c', 'user.name=Synthetic Fixture', '-c', 'user.email=fixture@example.invalid', 'commit', '-m', 'synthetic source');
  const commit = git('rev-parse', 'HEAD').trim();
  const output = join(temporary, 'receipt.json');
  writeFileSync(join(bin, 'pnpm'), `#!${process.execPath}
const fs = require('node:fs');
const path = 'src-tauri/.release-source-commit';
const receipt = { args: process.argv.slice(2), commit: process.env.MDREADER_RELEASE_SOURCE_COMMIT, jobs: process.env.CARGO_BUILD_JOBS, pid: process.pid };
receipt.runtimeFlags = Object.fromEntries(${JSON.stringify(RUNTIME_FLAGS)}.map(name => [name, process.env[name] ?? null]));
delete process.env.MDREADER_RELEASE_SOURCE_COMMIT;
receipt.nestedFile = fs.readFileSync(path, 'utf8');
fs.writeFileSync(process.env.FIXTURE_OUTPUT, JSON.stringify(receipt));
if (process.env.FIXTURE_MODE === 'drift') fs.writeFileSync('src-tauri/generated.plist', 'generated change\\n');
if (process.env.FIXTURE_MODE === 'replace') { fs.unlinkSync(path); fs.writeFileSync(path, receipt.nestedFile); }
if (process.env.FIXTURE_MODE === 'wait') setInterval(() => {}, 1000);
else process.exit(process.env.FIXTURE_MODE === 'failure' ? 23 : 0);
`);
  chmodSync(join(bin, 'pnpm'), 0o700);
  const env = { ...isolated, PATH: `${bin}:${process.env.PATH}`, FIXTURE_OUTPUT: output };
  delete env.MDREADER_RELEASE_SOURCE_COMMIT;
  for (const name of RUNTIME_FLAGS) delete env[name];
  const command = join(root, 'scripts/development/build-mobile.mjs');
  const handoff = join(root, 'src-tauri/.release-source-commit');
  const run = (mode, args = [], extra = {}, platform = 'ios') => spawnSync(process.execPath, [command, platform, ...args], {
    env: { ...env, FIXTURE_MODE: mode, ...extra }, encoding: 'utf8', timeout: 20_000,
  });
  return { root, command, commit, output, env, handoff, run, cleanup: () => rmSync(temporary, { recursive: true, force: true }) };
}

test('handoff is exclusive and cleanup preserves changed/replaced/symlink files', () => {
  const directory = mkdtempSync(join(tmpdir(), 'mdreader-handoff-'));
  const path = join(directory, '.release-source-commit');
  try {
    const cleanup = stageHandoff(path, COMMIT);
    assert.throws(() => stageHandoff(path, COMMIT), /exclusively create/);
    assert.equal(readFileSync(path, 'utf8'), `${COMMIT}\n`);
    cleanup();
    assert.equal(existsSync(path), false);
    for (const mode of ['changed', 'replaced', 'symlink']) {
      const clean = stageHandoff(path, COMMIT);
      if (mode === 'changed') writeFileSync(path, `${'a'.repeat(40)}\n`);
      else {
        unlinkSync(path);
        if (mode === 'replaced') writeFileSync(path, `${COMMIT}\n`);
        else { writeFileSync(join(directory, 'other'), `${COMMIT}\n`); symlinkSync(join(directory, 'other'), path); }
      }
      assert.throws(clean, /refusing to remove/);
      assert.equal(existsSync(path), true);
      unlinkSync(path);
    }
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test('mobile runtime restrictions refuse both platforms before child or handoff creation', () => {
  const f = fixture();
  try {
    for (const platform of ['ios', 'android']) {
      for (const name of RUNTIME_FLAGS) {
        for (const value of ['true', 'TRUE', '1']) {
          const result = f.run('success', [], { [name]: value }, platform);
          assert.equal(result.status, 1);
          assert.match(result.stderr, new RegExp(`${name} disables the pinned ONNX runtime`));
          assert.equal(existsSync(f.output), false, 'restricted build spawned fake pnpm');
          assert.equal(existsSync(f.handoff), false, 'restricted build left a handoff');
        }
      }
    }
    // A stale handoff would make stageHandoff throw a different error. The
    // resolver refusal wins and leaves it byte-identical: staging was not tried.
    writeFileSync(f.handoff, 'retained handoff\n');
    const result = f.run('success', [], { CARGO_NET_OFFLINE: 'false', ORT_OFFLINE: 'true', ORT_LIB_PATH: '/synthetic/runtime' });
    assert.match(result.stderr, /ORT_OFFLINE disables the pinned ONNX runtime/);
    assert.equal(readFileSync(f.handoff, 'utf8'), 'retained handoff\n');
    assert.equal(existsSync(f.output), false);
  } finally { f.cleanup(); }
});

test('direct API rejects nonstring runtime flags before inspecting a nonexistent checkout', async () => {
  const directory = mkdtempSync(join(tmpdir(), 'mdreader-mobile-env-'));
  const root = join(directory, 'nonexistent-checkout');
  try {
    for (const platform of ['ios', 'android']) {
      for (const name of RUNTIME_FLAGS) {
        for (const value of [1, true, 0, false, null, {}]) {
          await assert.rejects(runMobileBuild(platform, [], root, { [name]: value }), {
            message: `${name} must be a string or unset for a Tauri mobile build`,
          });
          assert.equal(existsSync(root), false, 'preflight touched the nonexistent checkout');
        }
      }
    }
  } finally { rmSync(directory, { recursive: true, force: true }); }
});

test('false and zero runtime flags remain unchanged in the mobile child environment', () => {
  const f = fixture();
  try {
    for (const platform of ['ios', 'android']) {
      for (const value of ['false', '0']) {
        const flags = Object.fromEntries(RUNTIME_FLAGS.map(name => [name, value]));
        const result = f.run('success', [], flags, platform);
        assert.equal(result.status, 0, result.stderr);
        const receipt = JSON.parse(readFileSync(f.output, 'utf8'));
        assert.deepEqual(receipt.runtimeFlags, flags);
        assert.deepEqual(receipt.args, ['tauri', platform, 'build']);
        assert.equal(receipt.nestedFile, `${f.commit}\n`);
        assert.equal(existsSync(f.handoff), false);
        unlinkSync(f.output);
      }
    }
  } finally { f.cleanup(); }
});

test('standalone wrapper binds clean HEAD through stripped nested environment and literal argv', () => {
  const f = fixture();
  try {
    const literal = '--a; $(do-not-execute)';
    const result = f.run('success', ['--debug', '--config', literal]);
    assert.equal(result.status, 0, result.stderr);
    const receipt = JSON.parse(readFileSync(f.output, 'utf8'));
    assert.deepEqual(receipt.args, ['tauri', 'ios', 'build', '--debug', '--config', literal]);
    assert.equal(receipt.commit, f.commit);
    assert.equal(receipt.nestedFile, `${f.commit}\n`);
    assert.equal(receipt.jobs, '2');
    assert.equal(existsSync(f.handoff), false);
  } finally { f.cleanup(); }
});

test('dirty source, conflicting identity, stale handoff and generated plist drift fail visibly', () => {
  const f = fixture();
  try {
    assert.notEqual(f.run('success', [], { MDREADER_RELEASE_SOURCE_COMMIT: COMMIT }).status, 0);
    assert.equal(existsSync(f.output), false);
    writeFileSync(join(f.root, 'untracked'), 'dirty');
    assert.match(f.run('success').stderr, /clean Git checkout/);
    unlinkSync(join(f.root, 'untracked'));
    writeFileSync(f.handoff, `${f.commit}\n`);
    assert.match(f.run('success').stderr, /exclusively create/);
    assert.equal(readFileSync(f.handoff, 'utf8'), `${f.commit}\n`);
    unlinkSync(f.handoff);
    assert.equal(existsSync(f.output), false);
    assert.match(f.run('drift').stderr, /clean Git checkout/);
    assert.equal(readFileSync(join(f.root, 'src-tauri/generated.plist'), 'utf8'), 'generated change\n');
    assert.equal(existsSync(f.handoff), false);
  } finally { f.cleanup(); }
});

test('child failures and interruption retain status, stop child, and clean only owned handoff', async () => {
  const f = fixture();
  try {
    assert.equal(f.run('failure').status, 23);
    assert.equal(existsSync(f.handoff), false);
    unlinkSync(f.output);
    const child = spawn(process.execPath, [f.command, 'android', '--debug'], {
      env: { ...f.env, FIXTURE_MODE: 'wait' }, stdio: 'ignore',
    });
    const exited = new Promise((done) => child.once('close', (code) => done(code)));
    try {
      for (let i = 0; i < 100 && !existsSync(f.output); i++) await pause(50);
      assert.equal(existsSync(f.output), true, 'fake child did not start');
      const receipt = JSON.parse(readFileSync(f.output, 'utf8'));
      assert.deepEqual(receipt.args, ['tauri', 'android', 'build', '--debug']);
      child.kill('SIGTERM');
      assert.equal(await exited, 143);
      assert.throws(() => process.kill(receipt.pid, 0), { code: 'ESRCH' });
      assert.equal(existsSync(f.handoff), false);
    } finally {
      child.kill('SIGTERM');
      if (existsSync(f.output)) {
        const { pid } = JSON.parse(readFileSync(f.output, 'utf8'));
        try { process.kill(-pid, 'SIGKILL'); }
        catch (error) { if (error.code !== 'ESRCH') throw error; }
      }
      child.kill('SIGKILL');
      await exited;
    }
    const replaced = f.run('replace');
    assert.equal(replaced.status, 1);
    assert.match(replaced.stderr, /refusing to remove replaced/);
    assert.equal(readFileSync(f.handoff, 'utf8'), `${f.commit}\n`);
  } finally { f.cleanup(); }
});
