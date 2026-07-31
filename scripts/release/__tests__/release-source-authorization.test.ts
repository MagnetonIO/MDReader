import { execFile } from 'node:child_process';
import {
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';

import { afterEach, describe, expect, test } from 'vitest';
import { parse } from 'yaml';

const execFileAsync = promisify(execFile);
const repositoryRoot = resolve(process.cwd(), '..', '..');
const roots: string[] = [];

async function git(cwd: string, ...args: string[]): Promise<string> {
  const { stdout } = await execFileAsync('git', args, { cwd });
  return stdout.trim();
}

function authorizationBody(source: string): string {
  const workflow = parse(source) as {
    jobs?: {
      prepare?: {
        steps?: Array<{ id?: string; run?: string }>;
      };
    };
  };
  const body = workflow.jobs?.prepare?.steps?.find(
    (step) => step.id === 'authorize-release-source',
  )?.run;
  expect(body).toBeTypeOf('string');
  return body!;
}

async function repositoryFixture() {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-source-authority-'));
  roots.push(root);
  const origin = join(root, 'origin.git');
  const checkout = join(root, 'checkout');
  await git(root, 'init', '--bare', origin);
  await mkdir(checkout);
  await git(checkout, 'init');
  await git(checkout, 'config', 'user.name', 'Release Authority Test');
  await git(
    checkout,
    'config',
    'user.email',
    'release-authority@example.invalid',
  );
  await mkdir(join(checkout, '.github', 'workflows'), { recursive: true });
  await writeFile(
    join(checkout, '.github', 'workflows', 'release.yml'),
    'name: historical-credentialed-workflow\n',
  );
  await git(checkout, 'add', '.github/workflows/release.yml');
  await git(checkout, 'commit', '-m', 'historical credentialed workflow');
  const historicalCommit = await git(checkout, 'rev-parse', 'HEAD');
  await writeFile(
    join(checkout, '.github', 'workflows', 'release.yml'),
    'name: current-approved-workflow\n',
  );
  await git(checkout, 'add', '.github/workflows/release.yml');
  await git(checkout, 'commit', '-m', 'current approved workflow');
  const currentCommit = await git(checkout, 'rev-parse', 'HEAD');
  await git(checkout, 'branch', '-M', 'main');
  await git(checkout, 'remote', 'add', 'origin', origin);
  await git(checkout, 'push', '-u', 'origin', 'main');
  return {
    checkout,
    historicalCommit,
    currentCommit,
  };
}

async function runAuthorization(input: {
  checkout: string;
  eventName: 'push' | 'workflow_dispatch';
  ref: string;
  sourceCommit: string;
}) {
  const source = await readFile(
    join(repositoryRoot, '.github', 'workflows', 'release.yml'),
    'utf8',
  );
  const script = join(input.checkout, 'authorize-release-source.sh');
  await writeFile(script, authorizationBody(source));
  return execFileAsync('bash', [script], {
    cwd: input.checkout,
    env: {
      ...process.env,
      GITHUB_EVENT_NAME: input.eventName,
      GITHUB_REF: input.ref,
      GITHUB_SHA: input.sourceCommit,
    },
  });
}

afterEach(async () => {
  await Promise.all(
    roots.splice(0).map((root) => rm(root, { recursive: true, force: true })),
  );
});

describe('release source authority', () => {
  test.each([
    ['push', 'refs/tags/mdreader-v1.0.0'],
    ['workflow_dispatch', 'refs/heads/main'],
  ] as const)(
    'accepts the exact current main tip selected by %s',
    async (eventName, ref) => {
      const fixture = await repositoryFixture();

      await expect(
        runAuthorization({
          ...fixture,
          eventName,
          ref,
          sourceCommit: fixture.currentCommit,
        }),
      ).resolves.toMatchObject({ stderr: '' });
    },
  );

  test.each([
    ['push', 'refs/tags/mdreader-v1.0.0'],
    ['workflow_dispatch', 'refs/heads/main'],
  ] as const)(
    'rejects a historical main ancestor selected by %s',
    async (eventName, ref) => {
      const fixture = await repositoryFixture();

      await expect(
        runAuthorization({
          ...fixture,
          eventName,
          ref,
          sourceCommit: fixture.historicalCommit,
        }),
      ).rejects.toThrow(/current origin\/main tip/i);
    },
  );
});
