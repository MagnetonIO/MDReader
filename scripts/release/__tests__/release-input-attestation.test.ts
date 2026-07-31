import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import { readFileSync } from 'node:fs';
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

import { attestReleaseBuildConfig } from '../release-build-attestation';
import { createReleaseConfig } from '../release-config';

const execFileAsync = promisify(execFile);
const repositoryRoot = resolve(process.cwd(), '..', '..');
const roots: string[] = [];
const cases = {
  macos: {
    target: 'universal-apple-darwin',
    bundles: 'dmg',
  },
  windows: {
    target: 'x86_64-pc-windows-msvc',
    bundles: 'nsis',
  },
  linux: {
    target: 'x86_64-unknown-linux-gnu',
    bundles: 'appimage,deb,rpm',
  },
} as const;

type Platform = keyof typeof cases;

function embeddedVerifierBody(): string {
  const workflow = parse(
    readFileSync(
      join(repositoryRoot, '.github', 'workflows', 'release.yml'),
      'utf8',
    ),
  ) as {
    jobs?: {
      'bundle-sign'?: {
        steps?: Array<{ id?: string; run?: string }>;
      };
    };
  };
  const verifier = workflow.jobs?.['bundle-sign']?.steps?.find(
    (step) => step.id === 'verify-credentialed-release-config',
  )?.run;
  if (typeof verifier !== 'string') {
    throw new Error(
      'release workflow has no embedded cross-platform release-input verifier',
    );
  }
  return verifier;
}

async function fixture(platform: Platform): Promise<{
  root: string;
  signerPath: string;
  hookPath: string;
  configPath: string;
  attestationPath: string;
  attestationSha256: string;
  macosFrameworkPath: string | null;
  macosFrameworkSha256: string | null;
  platform: Platform;
  target: string;
  bundles: string;
}> {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-release-boundary-'));
  roots.push(root);
  const releaseScripts = join(root, 'scripts', 'release');
  const signerPath = join(releaseScripts, 'sign-windows-from-store.ps1');
  const hookPath = join(releaseScripts, 'windows-minimum.nsh');
  const macosFrameworkPath =
    platform === 'macos'
      ? join(root, 'runtime', 'macos', 'libonnxruntime.1.24.2.dylib')
      : null;
  const macosFrameworkBody = Buffer.from('reviewed ONNX Runtime fixture');
  const configPath = join(root, 'release-config.json');
  const attestationPath = join(root, 'build-attestation.json');
  const expected = cases[platform];
  await mkdir(releaseScripts, { recursive: true });
  await writeFile(signerPath, 'Write-Output "reviewed signer"\n');
  await writeFile(hookPath, '!define MDREADER_MINIMUM_WINDOWS "10.0"\n');
  if (macosFrameworkPath !== null) {
    await mkdir(join(root, 'runtime', 'macos'), { recursive: true });
    await writeFile(macosFrameworkPath, macosFrameworkBody);
  }
  const config = createReleaseConfig({
    version: '1.0.0',
    workspaceVersion: '1.0.0',
    bundleIdentifier: 'dev.mdreader.app',
    updaterPublicKey: 'A'.repeat(88),
    updaterEndpoint:
      'https://downloads.mdreader.dev/channels/stable/updater.json',
    ...(platform === 'macos'
      ? {
          macosSigningIdentity:
            'Developer ID Application: Example (TEAMID)',
          macosFramework: macosFrameworkPath ?? undefined,
        }
      : {}),
    ...(platform === 'windows'
      ? {
          windowsCertificateThumbprint:
            '0123456789ABCDEF0123456789ABCDEF01234567',
          windowsInstallerHooks: hookPath,
          windowsSignCommand: signerPath,
        }
      : {}),
  });
  await writeFile(configPath, `${JSON.stringify(config, null, 2)}\n`);
  await attestReleaseBuildConfig(
    configPath,
    attestationPath,
    expected.target,
    expected.bundles,
  );
  const attestationSha256 = createHash('sha256')
    .update(await readFile(attestationPath))
    .digest('hex');
  return {
    root,
    signerPath,
    hookPath,
    configPath,
    attestationPath,
    attestationSha256,
    macosFrameworkPath,
    macosFrameworkSha256:
      macosFrameworkPath === null
        ? null
        : createHash('sha256').update(macosFrameworkBody).digest('hex'),
    platform,
    ...expected,
  };
}

async function runEmbeddedVerifier(input: Awaited<ReturnType<typeof fixture>>) {
  const verifierPath = join(input.root, 'verify-release-inputs.sh');
  await writeFile(verifierPath, embeddedVerifierBody());
  return execFileAsync('bash', [verifierPath], {
    env: {
      ...process.env,
      RELEASE_CONFIG_PATH: input.configPath,
      EXPECTED_RELEASE_VERSION: '1.0.0',
      EXPECTED_RELEASE_TARGET: input.target,
      EXPECTED_RELEASE_BUNDLES: input.bundles,
      EXPECTED_RELEASE_PLATFORM: input.platform,
      EXPECTED_UPDATER_PUBLIC_KEY_SHA256: createHash('sha256')
        .update('A'.repeat(88), 'utf8')
        .digest('hex'),
      WINDOWS_SIGNER_PATH: input.signerPath,
      WINDOWS_HOOK_PATH: input.hookPath,
      MACOS_ORT_DYLIB_PATH: input.macosFrameworkPath ?? '',
      EXPECTED_MACOS_ORT_DYLIB_SHA256:
        input.macosFrameworkSha256 ?? '',
      GITHUB_OUTPUT: join(input.root, 'github-output.txt'),
    },
  });
}

afterEach(async () => {
  await Promise.all(
    roots.splice(0).map((root) => rm(root, { recursive: true, force: true })),
  );
});

describe('credentialed release-config boundary', () => {
  test.each(['macos', 'linux', 'windows'] as const)(
    'accepts the exact %s release config before signing starts',
    async (platform) => {
      const input = await fixture(platform);

      await expect(runEmbeddedVerifier(input)).resolves.toMatchObject({
        stderr: '',
      });
    },
  );

  test.each(['macos', 'linux', 'windows'] as const)(
    'rejects a %s updater identity replacement before signing starts',
    async (platform) => {
      const input = await fixture(platform);
      const parsed = JSON.parse(await readFile(input.configPath, 'utf8'));
      parsed.plugins.updater.pubkey = 'B'.repeat(88);
      await writeFile(input.configPath, `${JSON.stringify(parsed, null, 2)}\n`);

      await expect(runEmbeddedVerifier(input)).rejects.toThrow(
        /wrong product or updater identity/i,
      );
    },
  );

  test('rejects a Windows signer path replacement before certificate import', async () => {
    const input = await fixture('windows');
    const parsed = JSON.parse(await readFile(input.configPath, 'utf8'));
    parsed.bundle.windows.signCommand.args[3] = join(
      input.root,
      'replacement-signer.ps1',
    );
    await writeFile(input.configPath, `${JSON.stringify(parsed, null, 2)}\n`);

    await expect(runEmbeddedVerifier(input)).rejects.toThrow(
      /Windows.*signer|reviewed NSIS/i,
    );
  });

  test('rejects a pre-attested Windows config whose required NSIS hook was removed', async () => {
    const input = await fixture('windows');
    const parsed = JSON.parse(await readFile(input.configPath, 'utf8'));
    delete parsed.bundle.windows.nsis;
    await writeFile(
      input.configPath,
      `${JSON.stringify(parsed, null, 2)}\n`,
    );
    await expect(
      attestReleaseBuildConfig(
        input.configPath,
        input.attestationPath,
        input.target,
        input.bundles,
      ),
    ).rejects.toThrow(
      /Windows.*NSIS|installer hook/i,
    );
  });

  test('rejects a Windows platform attested with the macOS target and bundle tuple', async () => {
    const input = await fixture('windows');
    input.target = 'universal-apple-darwin';
    input.bundles = 'dmg';
    await expect(
      attestReleaseBuildConfig(
        input.configPath,
        input.attestationPath,
        input.target,
        input.bundles,
      ),
    ).rejects.toThrow(
      /platform shape|target.*bundle.*tuple/i,
    );
  });
});
