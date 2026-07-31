import { execFile } from 'node:child_process';
import { readFileSync } from 'node:fs';
import { mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';

import { describe, expect, test } from 'vitest';
import { parse } from 'yaml';

const execFileAsync = promisify(execFile);
const root = resolve(process.cwd(), '..', '..');
const ownedStore =
  'MDReaderRelease-4242-3-0123456789ABCDEF0123456789ABCDEF';
const thumbprint = '0123456789ABCDEF0123456789ABCDEF01234567';

interface CleanupOutcome {
  outcome: 'succeeded' | 'failed' | 'spawn-error';
  message: string;
  removed: string[];
  storeExists: boolean;
  pfxExists: boolean;
}

function embeddedCleanupBody(): string {
  const workflow = parse(
    readFileSync(join(root, '.github', 'workflows', 'release.yml'), 'utf8'),
  ) as {
    jobs?: {
      'bundle-sign'?: {
        steps?: Array<{ id?: string; run?: string }>;
      };
    };
  };
  const cleanup = workflow.jobs?.['bundle-sign']?.steps?.find(
    (step) => step.id === 'cleanup-windows-cert',
  )?.run;
  if (typeof cleanup !== 'string') {
    throw new Error('release workflow has no embedded Windows cleanup body');
  }
  return cleanup;
}

async function runCleanup(input: {
  storeName?: string;
  owned?: string;
  storeExists: boolean;
  pfxExists?: boolean;
  failCertificateDelete?: boolean;
}): Promise<CleanupOutcome> {
  const directory = await mkdtemp(join(tmpdir(), 'mdreader-store-cleanup-'));
  const harness = join(directory, 'harness.ps1');
  const capture = join(directory, 'outcome.json');
  const pfx = join(directory, 'mdreader-release.pfx');
  const cleanup = join(directory, 'embedded-cleanup.ps1');
  try {
    await writeFile(cleanup, embeddedCleanupBody());
    await writeFile(
      harness,
      [
        '$global:StorePath = "Cert:\\LocalMachine\\$($env:WINDOWS_CERT_STORE)"',
        '$global:CertificatePath = "$global:StorePath\\$($env:FAKE_THUMBPRINT)"',
        "$global:StoreExists = $env:FAKE_STORE_EXISTS -eq 'true'",
        "$global:PfxExists = $env:FAKE_PFX_EXISTS -eq 'true'",
        '$global:Certificates = @([PSCustomObject]@{ Thumbprint = $env:FAKE_THUMBPRINT })',
        '$global:Removed = [Collections.Generic.List[string]]::new()',
        'function global:Test-Path {',
        '  param([string]$LiteralPath, [object]$PathType)',
        '  if ($LiteralPath -eq $global:StorePath) { return $global:StoreExists }',
        '  if ($LiteralPath -eq $env:WINDOWS_CERTIFICATE_PATH) { return $global:PfxExists }',
        '  return $false',
        '}',
        'function global:Get-ChildItem {',
        '  param([string]$LiteralPath, [object]$ErrorAction)',
        '  if ($LiteralPath -ne $global:StorePath -or -not $global:StoreExists) {',
        "    throw 'unexpected certificate-store enumeration'",
        '  }',
        '  return @($global:Certificates)',
        '}',
        'function global:Get-Item {',
        '  param([string]$LiteralPath, [object]$ErrorAction)',
        '  if ($LiteralPath -ne $global:CertificatePath) {',
        "    throw 'unexpected certificate lookup'",
        '  }',
        '  return [PSCustomObject]@{ Thumbprint = $env:FAKE_THUMBPRINT }',
        '}',
        'function global:Remove-Item {',
        '  param(',
        '    [string]$LiteralPath,',
        '    [switch]$DeleteKey,',
        '    [switch]$Force,',
        '    [object]$ErrorAction',
        '  )',
        '  if ($LiteralPath -eq $global:CertificatePath) {',
        "    if ($env:FAKE_FAIL_CERT_DELETE -eq 'true') { throw 'injected key deletion failure' }",
        '    $global:Removed.Add($LiteralPath)',
        '    $global:Certificates = @()',
        '    return',
        '  }',
        '  if ($LiteralPath -eq $global:StorePath) {',
        "    if ($global:Certificates.Count -ne 0) { throw 'store is not empty' }",
        '    $global:Removed.Add($LiteralPath)',
        '    $global:StoreExists = $false',
        '    return',
        '  }',
        '  if ($LiteralPath -eq $env:WINDOWS_CERTIFICATE_PATH) {',
        '    $global:Removed.Add($LiteralPath)',
        '    $global:PfxExists = $false',
        '    return',
        '  }',
        "  throw 'unexpected removal target'",
        '}',
        "$status = 'succeeded'",
        "$message = ''",
        'try {',
        '  & $env:CLEANUP_SCRIPT',
        '} catch {',
        "  $status = 'failed'",
        '  $message = $_.Exception.Message',
        '}',
        '[IO.File]::WriteAllText(',
        '  $env:CAPTURE_PATH,',
        '  (@{',
        '    outcome = $status',
        '    message = $message',
        '    removed = @($global:Removed)',
        '    storeExists = $global:StoreExists',
        '    pfxExists = $global:PfxExists',
        '  } | ConvertTo-Json -Compress)',
        ')',
      ].join('\n'),
    );
    try {
      await execFileAsync(
        'pwsh',
        ['-NoProfile', '-NonInteractive', '-File', harness],
        {
          env: {
            ...process.env,
            GITHUB_RUN_ID: '4242',
            GITHUB_RUN_ATTEMPT: '3',
            ...(input.storeName === undefined
              ? {}
              : { WINDOWS_CERT_STORE: input.storeName }),
            ...(input.owned === undefined
              ? {}
              : { WINDOWS_CERT_STORE_OWNED: input.owned }),
            WINDOWS_CERTIFICATE_PATH: pfx,
            FAKE_STORE_EXISTS: String(input.storeExists),
            FAKE_PFX_EXISTS: String(input.pfxExists ?? true),
            FAKE_FAIL_CERT_DELETE: String(
              input.failCertificateDelete ?? false,
            ),
            FAKE_THUMBPRINT: thumbprint,
            CLEANUP_SCRIPT: cleanup,
            CAPTURE_PATH: capture,
          },
        },
      );
    } catch (error) {
      return {
        outcome: 'spawn-error',
        message: error instanceof Error ? error.message : String(error),
        removed: [],
        storeExists: input.storeExists,
        pfxExists: input.pfxExists ?? true,
      };
    }
    return JSON.parse(await readFile(capture, 'utf8')) as CleanupOutcome;
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

describe('Windows release certificate-store ownership cleanup', () => {
  test('does not derive or delete a store when the import step emitted no capability', async () => {
    await expect(
      runCleanup({
        storeExists: true,
      }),
    ).resolves.toEqual({
      outcome: 'succeeded',
      message: '',
      removed: [expect.stringMatching(/mdreader-release\.pfx$/)],
      storeExists: true,
      pfxExists: false,
    });
  });

  test('never deletes a colliding store when atomic creation did not prove ownership', async () => {
    await expect(
      runCleanup({
        storeName: ownedStore,
        owned: '',
        storeExists: true,
      }),
    ).resolves.toEqual({
      outcome: 'failed',
      message: expect.stringMatching(/unproven|ownership|refus/i),
      removed: [expect.stringMatching(/mdreader-release\.pfx$/)],
      storeExists: true,
      pfxExists: false,
    });
  });

  test('accepts interruption after capability emission but before store creation', async () => {
    await expect(
      runCleanup({
        storeName: ownedStore,
        owned: '',
        storeExists: false,
      }),
    ).resolves.toEqual({
      outcome: 'succeeded',
      message: '',
      removed: [expect.stringMatching(/mdreader-release\.pfx$/)],
      storeExists: false,
      pfxExists: false,
    });
  });

  test('deletes keys and the store only after explicit creation ownership', async () => {
    const outcome = await runCleanup({
      storeName: ownedStore,
      owned: 'true',
      storeExists: true,
    });

    expect(outcome).toEqual({
      outcome: 'succeeded',
      message: '',
      removed: [
        `Cert:\\LocalMachine\\${ownedStore}\\${thumbprint}`,
        `Cert:\\LocalMachine\\${ownedStore}`,
        expect.stringMatching(/mdreader-release\.pfx$/),
      ],
      storeExists: false,
      pfxExists: false,
    });
  });

  test('fails closed and still removes the PFX when owned-key deletion fails', async () => {
    const outcome = await runCleanup({
      storeName: ownedStore,
      owned: 'true',
      storeExists: true,
      failCertificateDelete: true,
    });

    expect(outcome).toEqual({
      outcome: 'failed',
      message: expect.stringMatching(/deletion failure/),
      removed: [expect.stringMatching(/mdreader-release\.pfx$/)],
      storeExists: true,
      pfxExists: false,
    });
  });
});
