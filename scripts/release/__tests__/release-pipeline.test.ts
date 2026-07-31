import { createHash } from 'node:crypto';
import { execFileSync } from 'node:child_process';
import { mkdtemp, mkdir, rm, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import { join, resolve } from 'node:path';

import { afterEach, describe, expect, test } from 'vitest';

import {
  publicationPlan,
  validateReleaseDirectory,
  verifyUpdaterSignature,
} from '../release-pipeline';

const SOURCE_COMMIT = '0123456789abcdef0123456789abcdef01234567';
const PUBLISHED_AT = '2026-07-26T12:00:00.000Z';
const MINISIGN_PUBLIC_KEY = Buffer.from(
  'untrusted comment: minisign public key E7620F1842B4E81F\n' +
    'RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3\n',
).toString('base64');
const MINISIGN_SIGNATURE = Buffer.from(
  'untrusted comment: signature from minisign secret key\n' +
    'RWQf6LRCGA9i59SLOFxz6NxvASXDJeRtuZykwQepbDEGt87ig1BNpWaVWuNrm73YiIiJbq71Wi+dP9eKL8OC351vwIasSSbXxwA=\n' +
    'trusted comment: timestamp:1555779966\tfile:test\n' +
    'QtKMXWyYcwdpZAlPF7tE2ENJkRd1ujvKjlj1m9RtHTBnZPa5WKU5uWRs5GoP5M/VqE81QFuMKI5k/SfNQUaOAA==\n',
).toString('base64');
const SIGNATURE = MINISIGN_SIGNATURE;
const roots: string[] = [];

function sha256(contents: string): string {
  return createHash('sha256').update(contents).digest('hex');
}

function artifactSbom(filename: string, body: string): string {
  const artifactId = `SPDXRef-${filename.replace(/[^A-Za-z0-9.-]/g, '-')}`;
  return JSON.stringify({
    spdxVersion: 'SPDX-2.3',
    dataLicense: 'CC0-1.0',
    SPDXID: 'SPDXRef-DOCUMENT',
    name: `${filename} SBOM`,
    documentNamespace: `https://mdreader.dev/spdx/test/${filename}`,
    creationInfo: {
      created: PUBLISHED_AT,
      creators: ['Tool: syft-1.29.0'],
    },
    packages: [
      {
        SPDXID: artifactId,
        name: filename,
        checksums: [{ algorithm: 'SHA256', checksumValue: sha256(body) }],
      },
      {
        SPDXID: 'SPDXRef-runtime',
        name: 'runtime-dependency',
        versionInfo: '1.0.0',
      },
    ],
    relationships: [
      {
        spdxElementId: 'SPDXRef-DOCUMENT',
        relationshipType: 'DESCRIBES',
        relatedSpdxElement: artifactId,
      },
      {
        spdxElementId: artifactId,
        relationshipType: 'CONTAINS',
        relatedSpdxElement: 'SPDXRef-runtime',
      },
    ],
  });
}

function artifact(
  platform: 'macos' | 'windows' | 'linux',
  architecture: 'universal' | 'x86_64',
  kind: 'dmg' | 'nsis' | 'appimage' | 'deb' | 'rpm' | 'updater',
  filename: string,
  body: string,
  sbom: string,
) {
  return {
    channel: 'stable',
    version: '1.0.0',
    platform,
    architecture,
    kind,
    url: `https://downloads.mdreader.dev/releases/v1.0.0/${filename}`,
    bytes: Buffer.byteLength(body),
    sha256: sha256(body),
    minimumOs:
      platform === 'macos'
        ? 'macOS 12.0'
        : platform === 'windows'
          ? 'Windows 10'
          : 'glibc 2.31',
    signature: kind === 'updater' ? SIGNATURE : null,
    sbomUrl: `https://downloads.mdreader.dev/releases/v1.0.0/${filename}.spdx.json`,
    sbomSha256: sha256(sbom),
    publishedAt: PUBLISHED_AT,
    sourceCommit: SOURCE_COMMIT,
    updaterPublicKeySha256: sha256(MINISIGN_PUBLIC_KEY),
    releaseConfigSha256: 'd'.repeat(64),
  };
}

function releaseFixture() {
  const entries = [
    ['macos', 'universal', 'dmg', 'MDReader-1.0.0-universal.dmg'],
    ['macos', 'universal', 'updater', 'MDReader-1.0.0-universal.app.tar.gz'],
    ['windows', 'x86_64', 'nsis', 'MDReader-1.0.0-x64-setup.exe'],
    ['windows', 'x86_64', 'updater', 'MDReader-1.0.0-x64-setup.nsis.zip'],
    ['linux', 'x86_64', 'appimage', 'MDReader-1.0.0-x86_64.AppImage'],
    ['linux', 'x86_64', 'deb', 'MDReader-1.0.0-x86_64.deb'],
    ['linux', 'x86_64', 'rpm', 'MDReader-1.0.0-x86_64.rpm'],
    ['linux', 'x86_64', 'updater', 'MDReader-1.0.0-x86_64.AppImage.tar.gz'],
  ] as const;

  const files = entries.map(([platform, architecture, kind, filename], index) => {
    const body = kind === 'updater' ? 'test' : `artifact-${index}`;
    const sbom = artifactSbom(filename, body);
    return {
      filename,
      body,
      sbom,
      entry: artifact(platform, architecture, kind, filename, body, sbom),
    };
  });
  return {
    manifest: {
      schemaVersion: 1,
      generatedAt: PUBLISHED_AT,
      sourceCommit: SOURCE_COMMIT,
      updaterPublicKeySha256: sha256(MINISIGN_PUBLIC_KEY),
      releases: [
        {
          channel: 'stable',
          current: true,
          version: '1.0.0',
          artifacts: files.map((file) => file.entry),
        },
      ],
    },
    files,
  };
}

async function materializeFixture() {
  const root = await mkdtemp(join(tmpdir(), 'mdreader-release-'));
  roots.push(root);
  const fixture = releaseFixture();
  await mkdir(join(root, 'releases', 'v1.0.0'), { recursive: true });
  for (const file of fixture.files) {
    const base = join(root, 'releases', 'v1.0.0', file.filename);
    await writeFile(base, file.body);
    await writeFile(`${base}.spdx.json`, file.sbom);
    if (file.entry.kind === 'updater') await writeFile(`${base}.sig`, SIGNATURE);
  }
  return { root, ...fixture };
}

afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => rm(root, { recursive: true, force: true })));
});

describe('release publication boundary', () => {
  test('verifies the exact updater bytes with the exact public key embedded in the app', async () => {
    const root = await mkdtemp(join(tmpdir(), 'mdreader-updater-signature-'));
    roots.push(root);
    const artifact = join(root, 'updater.tar.gz');
    await writeFile(artifact, 'test');

    await expect(
      verifyUpdaterSignature(artifact, MINISIGN_SIGNATURE, MINISIGN_PUBLIC_KEY),
    ).resolves.toBeUndefined();
    await writeFile(artifact, 'Test');
    await expect(
      verifyUpdaterSignature(artifact, MINISIGN_SIGNATURE, MINISIGN_PUBLIC_KEY),
    ).rejects.toThrow(/signature verification failed/);
    await expect(
      verifyUpdaterSignature(artifact, MINISIGN_SIGNATURE, Buffer.from('wrong').toString('base64')),
    ).rejects.toThrow(/signature verification failed/);
  });

  test('verifies every local artifact, SBOM, and updater signature before planning uploads', async () => {
    const { root, manifest } = await materializeFixture();

    const verified = await validateReleaseDirectory(
      manifest,
      root,
      SOURCE_COMMIT,
      MINISIGN_PUBLIC_KEY,
    );
    const plan = publicationPlan(verified);

    expect(plan.filter((entry) => entry.kind === 'artifact')).toHaveLength(8);
    expect(plan.filter((entry) => entry.kind === 'sbom')).toHaveLength(8);
    expect(plan.filter((entry) => entry.kind === 'signature')).toHaveLength(3);
  });

  test('refuses a byte-identical filename whose contents do not match the manifest', async () => {
    const { root, manifest, files } = await materializeFixture();
    await writeFile(join(root, 'releases', 'v1.0.0', files[0]!.filename), 'tampered');

    await expect(
      validateReleaseDirectory(manifest, root, SOURCE_COMMIT, MINISIGN_PUBLIC_KEY),
    ).rejects.toThrow(
      /size|SHA-256/,
    );
  });

  test('refuses publication when an SBOM or updater signature is absent', async () => {
    const { root, manifest, files } = await materializeFixture();
    await rm(join(root, 'releases', 'v1.0.0', `${files[0]!.filename}.spdx.json`));
    await expect(
      validateReleaseDirectory(manifest, root, SOURCE_COMMIT, MINISIGN_PUBLIC_KEY),
    ).rejects.toThrow(/SBOM/);

    const second = await materializeFixture();
    const updater = second.files.find((file) => file.entry.kind === 'updater')!;
    await rm(join(second.root, 'releases', 'v1.0.0', `${updater.filename}.sig`));
    await expect(
      validateReleaseDirectory(
        second.manifest,
        second.root,
        SOURCE_COMMIT,
        MINISIGN_PUBLIC_KEY,
      ),
    ).rejects.toThrow(/signature/);
  });

  test('refuses source-SHA mismatch before producing a publication capability', async () => {
    const { root, manifest } = await materializeFixture();
    await expect(
      validateReleaseDirectory(
        manifest,
        root,
        'ffffffffffffffffffffffffffffffffffffffff',
        MINISIGN_PUBLIC_KEY,
      ),
    ).rejects.toThrow(/source commit/);
  });

  test('publishes immutable objects first, the stable manifest pointer next, and the updater pointer as the commit point', async () => {
    const { root, manifest } = await materializeFixture();
    const verified = await validateReleaseDirectory(
      manifest,
      root,
      SOURCE_COMMIT,
      MINISIGN_PUBLIC_KEY,
    );
    const plan = publicationPlan(verified);

    expect(plan.at(-3)).toMatchObject({
      kind: 'manifest',
      key: 'releases/v1.0.0/manifest.json',
    });
    expect(plan.at(-2)).toMatchObject({
      kind: 'pointer',
      key: 'channels/stable/manifest.json',
    });
    expect(plan.at(-1)).toMatchObject({
      kind: 'updater-manifest',
      key: 'channels/stable/updater.json',
    });
    expect(JSON.parse(plan.at(-1)!.contents!)).toMatchObject({
      version: '1.0.0',
      platforms: {
        'darwin-aarch64': {
          url: 'https://downloads.mdreader.dev/releases/v1.0.0/MDReader-1.0.0-universal.app.tar.gz',
          signature: SIGNATURE,
        },
        'darwin-x86_64': {
          url: 'https://downloads.mdreader.dev/releases/v1.0.0/MDReader-1.0.0-universal.app.tar.gz',
          signature: SIGNATURE,
        },
        'windows-x86_64': {
          url: 'https://downloads.mdreader.dev/releases/v1.0.0/MDReader-1.0.0-x64-setup.nsis.zip',
          signature: SIGNATURE,
        },
        'linux-x86_64': {
          url: 'https://downloads.mdreader.dev/releases/v1.0.0/MDReader-1.0.0-x86_64.AppImage.tar.gz',
          signature: SIGNATURE,
        },
      },
    });
    expect(plan.slice(0, -3).every((entry) => !['manifest', 'pointer', 'updater-manifest'].includes(entry.kind))).toBe(
      true,
    );
  });

  test('refuses to mint a publication plan when the embedded updater key differs', async () => {
    const { root, manifest } = await materializeFixture();
    await expect(
      validateReleaseDirectory(
        manifest,
        root,
        SOURCE_COMMIT,
        Buffer.from('wrong').toString('base64'),
      ),
    ).rejects.toThrow(/updater key fingerprint/);
  });

  test('refuses a manifest fingerprint not produced by the configured updater key', async () => {
    const { root, manifest } = await materializeFixture();
    manifest.updaterPublicKeySha256 = 'f'.repeat(64);
    await expect(
      validateReleaseDirectory(
        manifest,
        root,
        SOURCE_COMMIT,
        MINISIGN_PUBLIC_KEY,
      ),
    ).rejects.toThrow(/updater key fingerprint/);
  });

  test('the CLI dry run derives the complete R2 plan without credentials or writes', async () => {
    const { root, manifest } = await materializeFixture();
    const manifestPath = join(root, 'releases', 'v1.0.0', 'manifest.json');
    await writeFile(manifestPath, `${JSON.stringify(manifest, null, 2)}\n`);
    const repository = resolve(process.cwd(), '..', '..');

    const output = execFileSync(
      'pnpm',
      [
        'exec',
        'tsx',
        'scripts/release/release-cli.ts',
        'publish',
        '--root',
        root,
        '--manifest',
        manifestPath,
        '--source-commit',
        SOURCE_COMMIT,
        '--dry-run',
      ],
      {
        cwd: repository,
        encoding: 'utf8',
        env: {
          ...process.env,
          CLOUDFLARE_ACCOUNT_ID: '',
          MDREADER_R2_ACCESS_KEY_ID: '',
          MDREADER_R2_SECRET_ACCESS_KEY: '',
          MDREADER_UPDATER_PUBLIC_KEY: MINISIGN_PUBLIC_KEY,
        },
      },
    );

    expect(output).toContain(
      'DRY RUN artifact releases/v1.0.0/MDReader-1.0.0-universal.dmg',
    );
    expect(output).toContain(
      'DRY RUN manifest releases/v1.0.0/manifest.json',
    );
    expect(
      output
        .trim()
        .endsWith('DRY RUN updater-manifest channels/stable/updater.json'),
    ).toBe(true);
  });
});
