import { execFile } from 'node:child_process';
import { readFileSync } from 'node:fs';
import {
  copyFile,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';
import { promisify } from 'node:util';

import { describe, expect, test } from 'vitest';
import { parse } from 'yaml';

const execFileAsync = promisify(execFile);
const root = resolve(process.cwd(), '..', '..');

function embeddedStepBody(id: string): string {
  const workflow = parse(
    readFileSync(
      join(root, '.github', 'workflows', 'release.yml'),
      'utf8',
    ),
  ) as {
    jobs?: {
      'bundle-sign'?: {
        steps?: Array<{ id?: string; run?: string }>;
      };
    };
  };
  const body = workflow.jobs?.['bundle-sign']?.steps?.find(
    (step) => step.id === id,
  )?.run;
  if (typeof body !== 'string') {
    throw new Error(`release workflow has no embedded ${id} body`);
  }
  return body;
}

async function removeOwnedStoreFallback(storeName: string): Promise<void> {
  if (!/^MDReaderRelease-4242-3-[0-9A-F]{32}$/.test(storeName)) return;
  await execFileAsync(
    'pwsh',
    [
      '-NoProfile',
      '-NonInteractive',
      '-Command',
      [
        "$ErrorActionPreference = 'Stop'",
        '$path = "Cert:\\LocalMachine\\$env:WINDOWS_CERT_STORE"',
        'if (Test-Path -LiteralPath $path) {',
        '  Get-ChildItem -LiteralPath $path |',
        '    Remove-Item -DeleteKey -Force -ErrorAction Stop',
        '  Remove-Item -LiteralPath $path -Force -ErrorAction Stop',
        '}',
      ].join('\n'),
    ],
    {
      env: {
        ...process.env,
        WINDOWS_CERT_STORE: storeName,
      },
    },
  );
}

describe('Windows production certificate-provider contract', () => {
  test.skipIf(process.platform !== 'win32')(
    'creates one LocalMachine store, retains it on collision, signs through /sm, and removes only the owned store',
    async () => {
      const directory = await mkdtemp(
        join(tmpdir(), 'mdreader-provider-contract-'),
      );
      const pfxPath = join(directory, 'mdreader-release.pfx');
      const outputPath = join(directory, 'github-output.txt');
      const importPath = join(directory, 'import.ps1');
      const cleanupPath = join(directory, 'cleanup.ps1');
      const targetPath = join(directory, 'MDReader-provider-test.exe');
      let storeName = '';
      try {
        const password = 'temporary-provider-test-password';
        const generated = await execFileAsync(
          'pwsh',
          [
            '-NoProfile',
            '-NonInteractive',
            '-Command',
            [
              '$rsa = [Security.Cryptography.RSA]::Create(2048)',
              '$request = [Security.Cryptography.X509Certificates.CertificateRequest]::new(',
              "  'CN=MDReader release provider test',",
              '  $rsa,',
              '  [Security.Cryptography.HashAlgorithmName]::SHA256,',
              '  [Security.Cryptography.RSASignaturePadding]::Pkcs1',
              ')',
              '$oids = [Security.Cryptography.OidCollection]::new()',
              "$oids.Add([Security.Cryptography.Oid]::new('1.3.6.1.5.5.7.3.3')) | Out-Null",
              '$request.CertificateExtensions.Add(',
              '  [Security.Cryptography.X509Certificates.X509EnhancedKeyUsageExtension]::new($oids, $false)',
              ')',
              '$certificate = $request.CreateSelfSigned(',
              '  [DateTimeOffset]::UtcNow.AddMinutes(-5),',
              '  [DateTimeOffset]::UtcNow.AddHours(1)',
              ')',
              'try {',
              '  @{',
              `    base64 = [Convert]::ToBase64String($certificate.Export([Security.Cryptography.X509Certificates.X509ContentType]::Pfx, '${password}'))`,
              '    thumbprint = $certificate.Thumbprint',
              '  } | ConvertTo-Json -Compress',
              '} finally {',
              '  $certificate.Dispose()',
              '  $rsa.Dispose()',
              '}',
            ].join('\n'),
          ],
        );
        const certificate = JSON.parse(generated.stdout.trim()) as {
          base64: string;
          thumbprint: string;
        };
        await writeFile(importPath, embeddedStepBody('import-windows-cert'));
        await writeFile(
          cleanupPath,
          embeddedStepBody('cleanup-windows-cert'),
        );
        await execFileAsync(
          'pwsh',
          ['-NoProfile', '-NonInteractive', '-File', importPath],
          {
            env: {
              ...process.env,
              GITHUB_RUN_ID: '4242',
              GITHUB_RUN_ATTEMPT: '3',
              GITHUB_OUTPUT: outputPath,
              RUNNER_TEMP: directory,
              WINDOWS_CERTIFICATE: certificate.base64,
              WINDOWS_CERTIFICATE_PASSWORD: password,
              WINDOWS_CERT_THUMBPRINT: certificate.thumbprint,
            },
          },
        );
        const outputs = Object.fromEntries(
          (await readFile(outputPath, 'utf8'))
            .trim()
            .split(/\r?\n/)
            .map((line) => line.split('=', 2) as [string, string]),
        );
        storeName = outputs.store_name ?? '';
        expect(storeName).toMatch(
          /^MDReaderRelease-4242-3-[0-9A-F]{32}$/,
        );
        expect(outputs.store_owned).toBe('true');

        await expect(
          execFileAsync(
            'pwsh',
            [
              '-NoProfile',
              '-NonInteractive',
              '-Command',
              'New-Item -Path Cert:\\LocalMachine -Name $env:WINDOWS_CERT_STORE -ItemType Store -ErrorAction Stop',
            ],
            {
              env: {
                ...process.env,
                WINDOWS_CERT_STORE: storeName,
              },
            },
          ),
        ).rejects.toThrow();

        const persisted = await execFileAsync(
          'pwsh',
          [
            '-NoProfile',
            '-NonInteractive',
            '-Command',
            '(Get-Item -LiteralPath "Cert:\\LocalMachine\\$env:WINDOWS_CERT_STORE\\$env:WINDOWS_CERT_THUMBPRINT" -ErrorAction Stop).Thumbprint',
          ],
          {
            env: {
              ...process.env,
              WINDOWS_CERT_STORE: storeName,
              WINDOWS_CERT_THUMBPRINT: certificate.thumbprint,
            },
          },
        );
        expect(persisted.stdout.trim()).toBe(certificate.thumbprint);

        await copyFile(process.execPath, targetPath);
        await execFileAsync(
          'pwsh',
          [
            '-NoProfile',
            '-NonInteractive',
            '-File',
            join(root, 'scripts', 'release', 'sign-windows-from-store.ps1'),
            targetPath,
          ],
          {
            env: {
              ...process.env,
              GITHUB_RUN_ID: '4242',
              GITHUB_RUN_ATTEMPT: '3',
              WINDOWS_CERT_STORE: storeName,
              WINDOWS_CERT_STORE_OWNED: 'true',
              WINDOWS_CERT_THUMBPRINT: certificate.thumbprint,
            },
          },
        );
        const signed = await execFileAsync(
          'pwsh',
          [
            '-NoProfile',
            '-NonInteractive',
            '-Command',
            '(Get-AuthenticodeSignature -LiteralPath $env:SIGNED_TARGET).SignerCertificate.Thumbprint',
          ],
          {
            env: {
              ...process.env,
              SIGNED_TARGET: targetPath,
            },
          },
        );
        expect(signed.stdout.trim()).toBe(certificate.thumbprint);

        await execFileAsync(
          'pwsh',
          ['-NoProfile', '-NonInteractive', '-File', cleanupPath],
          {
            env: {
              ...process.env,
              GITHUB_RUN_ID: '4242',
              GITHUB_RUN_ATTEMPT: '3',
              WINDOWS_CERT_STORE: storeName,
              WINDOWS_CERT_STORE_OWNED: 'true',
              WINDOWS_CERTIFICATE_PATH: pfxPath,
            },
          },
        );
        const residue = await execFileAsync(
          'pwsh',
          [
            '-NoProfile',
            '-NonInteractive',
            '-Command',
            'Test-Path -LiteralPath "Cert:\\LocalMachine\\$env:WINDOWS_CERT_STORE"',
          ],
          {
            env: {
              ...process.env,
              WINDOWS_CERT_STORE: storeName,
            },
          },
        );
        expect(residue.stdout.trim()).toBe('False');
        storeName = '';
      } finally {
        if (storeName) await removeOwnedStoreFallback(storeName);
        await rm(directory, { recursive: true, force: true });
      }
    },
    120_000,
  );
});
