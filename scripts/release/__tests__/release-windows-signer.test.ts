import { execFile } from 'node:child_process';
import { chmod, mkdtemp, readFile, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';

import { describe, expect, test } from 'vitest';

const execFileAsync = promisify(execFile);
const root = resolve(process.cwd(), '..', '..');
const ownedStore =
  'MDReaderRelease-4242-3-0123456789ABCDEF0123456789ABCDEF';

async function runSigner(storeOwned?: string): Promise<string[]> {
  const directory = await mkdtemp(join(tmpdir(), 'mdreader-windows-sign-'));
  const target = join(directory, 'MDReader.exe');
  const captured = join(directory, 'signtool-args.txt');
  const goodSignTool = join(directory, 'resolved-signtool.exe');
  const harness = join(directory, 'harness.ps1');
  const signer = join(
    root,
    'scripts',
    'release',
    'sign-windows-from-store.ps1',
  );
  try {
    await writeFile(target, 'unsigned fixture');
    await writeFile(
      goodSignTool,
      '#!/usr/bin/env bash\nprintf "%s\\n" "$@" > "$CAPTURED_ARGS"\n',
    );
    await chmod(goodSignTool, 0o700);
    await writeFile(
      harness,
      [
        'function global:Get-Item {',
        '  param([string]$LiteralPath, [object]$ErrorAction)',
        "  if ($LiteralPath -like 'Cert:\\LocalMachine\\*') {",
        '    return [PSCustomObject]@{',
        '      Thumbprint = $env:WINDOWS_CERT_THUMBPRINT',
        '      HasPrivateKey = $true',
        '    }',
        '  }',
        '  Microsoft.PowerShell.Management\\Get-Item -LiteralPath $LiteralPath -ErrorAction $ErrorAction',
        '}',
        'function global:Get-Command {',
        '  param([string]$Name, [object]$ErrorAction)',
        "  if ($Name -eq 'signtool.exe') {",
        '    return [PSCustomObject]@{',
        '      Source = $env:RESOLVED_SIGNTOOL',
        "      Name = 'unreviewed-path-signtool.exe'",
        '    }',
        '  }',
        '  Microsoft.PowerShell.Core\\Get-Command $Name -ErrorAction $ErrorAction',
        '}',
        '& $env:SIGNER_PATH $env:TARGET_PATH',
      ].join('\n'),
    );
    await execFileAsync(
      'pwsh',
      ['-NoProfile', '-NonInteractive', '-File', harness],
      {
        env: {
          ...process.env,
          GITHUB_RUN_ID: '4242',
          GITHUB_RUN_ATTEMPT: '3',
          WINDOWS_CERT_STORE: ownedStore,
          ...(storeOwned === undefined
            ? {}
            : { WINDOWS_CERT_STORE_OWNED: storeOwned }),
          WINDOWS_CERT_THUMBPRINT:
            '0123456789ABCDEF0123456789ABCDEF01234567',
          CAPTURED_ARGS: captured,
          RESOLVED_SIGNTOOL: goodSignTool,
          SIGNER_PATH: signer,
          TARGET_PATH: target,
        },
      },
    );
    return (await readFile(captured, 'utf8')).trim().split(/\r?\n/);
  } finally {
    await rm(directory, { recursive: true, force: true });
  }
}

describe('Windows per-run certificate-store signer', () => {
  test('passes the owned store and inspected thumbprint to signtool', async () => {
    await expect(runSigner('true')).resolves.toEqual([
        'sign',
        '/sha1',
        '0123456789ABCDEF0123456789ABCDEF01234567',
        '/sm',
        '/s',
        ownedStore,
        '/fd',
        'SHA256',
        '/tr',
        'http://timestamp.digicert.com',
        '/td',
        'SHA256',
        expect.stringMatching(/MDReader\.exe$/),
      ]);
  });

  test('refuses to sign when atomic store-creation ownership is absent', async () => {
    await expect(runSigner()).rejects.toThrow(/owned store|ownership/i);
  });
});
