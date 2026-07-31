import { createHash } from 'node:crypto';

import { describe, expect, test } from 'vitest';

import type { PublicationEntry } from '../release-pipeline';
import {
  createSealedPublicationPlan,
  parseSealedPublicationPlan,
  sealedPublicationPlanSha256,
} from '../sealed-publication';

const SOURCE = '0123456789abcdef0123456789abcdef01234567';
const PUBLISHED_AT = '2026-07-26T12:00:00.000Z';
const SIGNATURE = 'U'.repeat(88);

function sha256(contents: string | Uint8Array): string {
  return createHash('sha256').update(contents).digest('hex');
}

function entries(version = '1.10.0'): PublicationEntry[] {
  const specifications = [
    ['macos', 'universal', 'dmg', `MDReader-${version}-universal.dmg`],
    [
      'macos',
      'universal',
      'updater',
      `MDReader-${version}-universal.app.tar.gz`,
    ],
    ['windows', 'x86_64', 'nsis', `MDReader-${version}-x64-setup.exe`],
    [
      'windows',
      'x86_64',
      'updater',
      `MDReader-${version}-x64-setup.nsis.zip`,
    ],
    ['linux', 'x86_64', 'appimage', `MDReader-${version}-x86_64.AppImage`],
    ['linux', 'x86_64', 'deb', `MDReader-${version}-x86_64.deb`],
    ['linux', 'x86_64', 'rpm', `MDReader-${version}-x86_64.rpm`],
    [
      'linux',
      'x86_64',
      'updater',
      `MDReader-${version}-x86_64.AppImage.tar.gz`,
    ],
  ] as const;
  const artifacts = specifications.map(
    ([platform, architecture, kind, filename], index) => {
      const body = `artifact-${index}`;
      const sbom = `{"spdxVersion":"SPDX-2.3","name":"${filename}"}\n`;
      return {
        body,
        sbom,
        manifest: {
          channel: 'stable',
          version,
          platform,
          architecture,
          kind,
          url: `https://downloads.mdreader.dev/releases/v${version}/${filename}`,
          bytes: Buffer.byteLength(body),
          sha256: sha256(body),
          minimumOs:
            platform === 'macos'
              ? 'macOS 12.0'
              : platform === 'windows'
                ? 'Windows 10'
                : 'glibc 2.31',
          signature: kind === 'updater' ? SIGNATURE : null,
          sbomUrl: `https://downloads.mdreader.dev/releases/v${version}/${filename}.spdx.json`,
          sbomSha256: sha256(sbom),
          publishedAt: PUBLISHED_AT,
          sourceCommit: SOURCE,
          updaterPublicKeySha256: 'c'.repeat(64),
          releaseConfigSha256: 'd'.repeat(64),
        },
      };
    },
  );
  const manifest = {
    schemaVersion: 1,
    generatedAt: PUBLISHED_AT,
    sourceCommit: SOURCE,
    updaterPublicKeySha256: 'c'.repeat(64),
    releases: [
      {
        channel: 'stable',
        current: true,
        version,
        artifacts: artifacts.map((entry) => entry.manifest),
      },
    ],
  };
  const manifestContents = `${JSON.stringify(manifest, null, 2)}\n`;
  const result: PublicationEntry[] = [];
  for (const artifact of artifacts) {
    const key = new URL(artifact.manifest.url).pathname.slice(1);
    result.push(
      {
        kind: 'artifact',
        key,
        immutable: true,
        localPath: null,
        contents: artifact.body,
      },
      {
        kind: 'sbom',
        key: new URL(artifact.manifest.sbomUrl).pathname.slice(1),
        immutable: true,
        localPath: null,
        contents: artifact.sbom,
      },
    );
    if (artifact.manifest.kind === 'updater') {
      result.push({
        kind: 'signature',
        key: `${key}.sig`,
        immutable: true,
        localPath: null,
        contents: `${SIGNATURE}\n`,
      });
    }
  }
  const updaterArtifacts = artifacts.filter(
    (entry) => entry.manifest.kind === 'updater',
  );
  const updater = (platform: 'macos' | 'windows' | 'linux') =>
    updaterArtifacts.find((entry) => entry.manifest.platform === platform)!
      .manifest;
  result.push(
    {
      kind: 'manifest',
      key: `releases/v${version}/manifest.json`,
      immutable: true,
      localPath: null,
      contents: manifestContents,
    },
    {
      kind: 'pointer',
      key: 'channels/stable/manifest.json',
      immutable: false,
      localPath: null,
      contents: `${JSON.stringify(
        {
          schemaVersion: 1,
          version,
          manifestUrl: `https://downloads.mdreader.dev/releases/v${version}/manifest.json`,
          manifestSha256: sha256(manifestContents),
          sourceCommit: SOURCE,
        },
        null,
        2,
      )}\n`,
    },
    {
      kind: 'updater-manifest',
      key: 'channels/stable/updater.json',
      immutable: false,
      localPath: null,
      contents: `${JSON.stringify(
        {
          version,
          notes: '',
          pub_date: PUBLISHED_AT,
          platforms: {
            'darwin-aarch64': {
              signature: SIGNATURE,
              url: updater('macos').url,
            },
            'darwin-x86_64': {
              signature: SIGNATURE,
              url: updater('macos').url,
            },
            'windows-x86_64': {
              signature: SIGNATURE,
              url: updater('windows').url,
            },
            'linux-x86_64': {
              signature: SIGNATURE,
              url: updater('linux').url,
            },
          },
        },
        null,
        2,
      )}\n`,
    },
  );
  return result;
}

async function seal(version = '1.10.0') {
  return createSealedPublicationPlan(
    entries(version),
    SOURCE,
    version,
    async (entry) => Buffer.from(entry.contents ?? '', 'utf8'),
  );
}

function replaceBody(
  plan: Awaited<ReturnType<typeof seal>>,
  key: string,
  body: string,
): void {
  const entry = plan.entries.find((candidate) => candidate.key === key)!;
  entry.bodyBase64 = Buffer.from(body).toString('base64');
  entry.bytes = Buffer.byteLength(body);
  entry.sha256 = sha256(body);
}

describe('sealed R2 publication capability', () => {
  test('contains only body-complete hashed entries ending in both stable pointers', async () => {
    const plan = await seal();
    expect(plan.entries).toHaveLength(22);
    expect(plan.entries.slice(-3).map((entry) => entry.key)).toEqual([
      'releases/v1.10.0/manifest.json',
      'channels/stable/manifest.json',
      'channels/stable/updater.json',
    ]);
    expect(
      plan.entries.every(
        (entry) =>
          entry.bodyBase64.length > 0 &&
          /^[0-9a-f]{64}$/.test(entry.sha256) &&
          entry.bytes > 0,
      ),
    ).toBe(true);
    const encoded = `${JSON.stringify(plan, null, 2)}\n`;
    expect(sealedPublicationPlanSha256(encoded)).toMatch(/^[0-9a-f]{64}$/);
    expect(parseSealedPublicationPlan(JSON.parse(encoded))).toEqual(plan);
  });

  test.each(['1.0.0-alpha', '1.0.0-A.2', '1.0.0+build.7'])(
    'rejects non-stable plan version %s',
    async (version) => {
      await expect(seal(version)).rejects.toThrow(/stable SemVer/);
    },
  );

  test('rejects unknown fields, body tampering, unsafe keys, and divergent pointers', async () => {
    const unknown = (await seal()) as unknown as Record<string, unknown>;
    unknown.unreviewed = true;
    expect(() => parseSealedPublicationPlan(unknown)).toThrow(/unknown/);

    const tampered = structuredClone(await seal());
    tampered.entries[0]!.bodyBase64 = Buffer.from('different').toString(
      'base64',
    );
    expect(() => parseSealedPublicationPlan(tampered)).toThrow(
      /byte length|SHA-256/,
    );

    const unsafe = structuredClone(await seal());
    unsafe.entries[0]!.key = 'releases/v1.10.0/../other';
    expect(() => parseSealedPublicationPlan(unsafe)).toThrow(/noncanonical/);

    const divergent = structuredClone(await seal());
    replaceBody(
      divergent,
      'channels/stable/manifest.json',
      '{"version":"9.0.0"}\n',
    );
    expect(() => parseSealedPublicationPlan(divergent)).toThrow(
      /divergent version|SHA-256/,
    );
  });

  test('rejects a structurally valid three-entry plan that omits the release matrix', async () => {
    const plan = await seal();
    plan.entries = plan.entries.slice(-3);
    expect(() => parseSealedPublicationPlan(plan)).toThrow(
      /complete release matrix|artifact|SBOM|signature/,
    );
  });

  test.each([
    ['artifact', 'MDReader-1.10.0-universal.dmg'],
    ['SBOM', 'MDReader-1.10.0-universal.dmg.spdx.json'],
    ['signature', 'MDReader-1.10.0-universal.app.tar.gz.sig'],
  ])('rejects a plan missing one required %s', async (_kind, suffix) => {
    const plan = await seal();
    plan.entries = plan.entries.filter((entry) => !entry.key.endsWith(suffix));
    expect(() => parseSealedPublicationPlan(plan)).toThrow(
      /complete release matrix|artifact|SBOM|signature/,
    );
  });

  test('rejects an unreferenced immutable object even when its hash is valid', async () => {
    const plan = await seal();
    const body = Buffer.from('unreviewed artifact');
    plan.entries.splice(-3, 0, {
      kind: 'artifact',
      key: 'releases/v1.10.0/unreviewed.bin',
      immutable: true,
      bytes: body.byteLength,
      sha256: sha256(body),
      bodyBase64: body.toString('base64'),
    });
    expect(() => parseSealedPublicationPlan(plan)).toThrow(
      /unreferenced|exact publication entries/,
    );
  });

  test('rejects a stable manifest pointer not bound to the immutable manifest', async () => {
    const plan = await seal();
    replaceBody(
      plan,
      'channels/stable/manifest.json',
      `${JSON.stringify({
        schemaVersion: 1,
        version: '1.10.0',
        manifestUrl:
          'https://downloads.mdreader.dev/releases/v1.10.0/wrong.json',
        manifestSha256: 'f'.repeat(64),
        sourceCommit: SOURCE,
      })}\n`,
    );
    expect(() => parseSealedPublicationPlan(plan)).toThrow(
      /manifest pointer|canonical|immutable manifest/,
    );
  });

  test('rejects an updater pointer missing a platform or changing its URL', async () => {
    const plan = await seal();
    const pointer = JSON.parse(
      Buffer.from(
        plan.entries.find(
          (entry) => entry.key === 'channels/stable/updater.json',
        )!.bodyBase64,
        'base64',
      ).toString('utf8'),
    ) as Record<string, unknown>;
    const platforms = pointer.platforms as Record<string, unknown>;
    delete platforms['windows-x86_64'];
    replaceBody(
      plan,
      'channels/stable/updater.json',
      `${JSON.stringify(pointer)}\n`,
    );
    expect(() => parseSealedPublicationPlan(plan)).toThrow(
      /updater pointer|platform/,
    );
  });

  test('rejects divergent manifest, artifact, and updater publication timestamps', async () => {
    const plan = await seal();
    const manifestEntry = plan.entries.find(
      (entry) => entry.key === 'releases/v1.10.0/manifest.json',
    )!;
    const manifest = JSON.parse(
      Buffer.from(manifestEntry.bodyBase64, 'base64').toString('utf8'),
    ) as {
      releases: Array<{ artifacts: Array<{ publishedAt: string }> }>;
    };
    for (const artifact of manifest.releases[0]!.artifacts) {
      artifact.publishedAt = '2026-07-26T12:00:01.000Z';
    }
    replaceBody(
      plan,
      'releases/v1.10.0/manifest.json',
      `${JSON.stringify(manifest, null, 2)}\n`,
    );

    const manifestPointerEntry = plan.entries.find(
      (entry) => entry.key === 'channels/stable/manifest.json',
    )!;
    const manifestPointer = JSON.parse(
      Buffer.from(manifestPointerEntry.bodyBase64, 'base64').toString('utf8'),
    ) as Record<string, unknown>;
    manifestPointer.manifestSha256 = plan.entries.find(
      (entry) => entry.key === 'releases/v1.10.0/manifest.json',
    )!.sha256;
    replaceBody(
      plan,
      'channels/stable/manifest.json',
      `${JSON.stringify(manifestPointer)}\n`,
    );
    const updaterPointerEntry = plan.entries.find(
      (entry) => entry.key === 'channels/stable/updater.json',
    )!;
    const updaterPointer = JSON.parse(
      Buffer.from(updaterPointerEntry.bodyBase64, 'base64').toString('utf8'),
    ) as Record<string, unknown>;
    updaterPointer.pub_date = '2026-07-26T12:00:01.000Z';
    replaceBody(
      plan,
      'channels/stable/updater.json',
      `${JSON.stringify(updaterPointer)}\n`,
    );

    expect(() => parseSealedPublicationPlan(plan)).toThrow(
      /canonical publication timestamp|publishedAt|generatedAt/,
    );
  });

  test('refuses an unbounded entry set', async () => {
    const plan = await seal();
    plan.entries = Array.from({ length: 65 }, (_, index) => ({
      ...plan.entries[0]!,
      key: `releases/v1.10.0/file-${index}`,
    }));
    expect(() => parseSealedPublicationPlan(plan)).toThrow(/too many|bounded/);
  });
});
