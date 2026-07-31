import { describe, expect, test } from 'vitest';

import { parseReleaseManifest } from '../lib/release-manifest';
import {
  downloadCopy,
  downloadCardsFor,
  downloadManifestFromRelease,
} from '../lib/downloads';

const SOURCE_COMMIT = '0123456789abcdef0123456789abcdef01234567';
const SHA_A = 'a'.repeat(64);
const SHA_B = 'b'.repeat(64);
const SIGNATURE = 'U'.repeat(88);
const PUBLISHED_AT = '2026-07-26T12:00:00.000Z';

function artifact(
  platform: 'macos' | 'windows' | 'linux',
  architecture: 'universal' | 'x86_64',
  kind: 'dmg' | 'nsis' | 'appimage' | 'deb' | 'rpm' | 'updater',
  filename: string,
) {
  return {
    channel: 'stable',
    version: '1.0.0',
    platform,
    architecture,
    kind,
    url: `https://downloads.mdreader.dev/releases/v1.0.0/${filename}`,
    bytes: 1_024,
    sha256: SHA_A,
    minimumOs:
      platform === 'macos'
        ? 'macOS 12.0'
        : platform === 'windows'
          ? 'Windows 10'
          : 'glibc 2.31',
    signature: kind === 'updater' ? SIGNATURE : null,
    sbomUrl: `https://downloads.mdreader.dev/releases/v1.0.0/${filename}.spdx.json`,
    sbomSha256: SHA_B,
    publishedAt: PUBLISHED_AT,
    sourceCommit: SOURCE_COMMIT,
    updaterPublicKeySha256: 'c'.repeat(64),
    releaseConfigSha256: 'd'.repeat(64),
  };
}

function validManifest(): unknown {
  return {
    schemaVersion: 1,
    generatedAt: PUBLISHED_AT,
    sourceCommit: SOURCE_COMMIT,
    updaterPublicKeySha256: 'c'.repeat(64),
    releases: [
      {
        channel: 'stable',
        current: true,
        version: '1.0.0',
        artifacts: [
          artifact('macos', 'universal', 'dmg', 'MDReader-1.0.0-universal.dmg'),
          artifact('macos', 'universal', 'updater', 'MDReader-1.0.0-universal.app.tar.gz'),
          artifact('windows', 'x86_64', 'nsis', 'MDReader-1.0.0-x64-setup.exe'),
          artifact('windows', 'x86_64', 'updater', 'MDReader-1.0.0-x64-setup.nsis.zip'),
          artifact('linux', 'x86_64', 'appimage', 'MDReader-1.0.0-x86_64.AppImage'),
          artifact('linux', 'x86_64', 'deb', 'MDReader-1.0.0-x86_64.deb'),
          artifact('linux', 'x86_64', 'rpm', 'MDReader-1.0.0-x86_64.rpm'),
          artifact('linux', 'x86_64', 'updater', 'MDReader-1.0.0-x86_64.AppImage.tar.gz'),
        ],
      },
    ],
  };
}

function cloneManifest(): Record<string, unknown> {
  return structuredClone(validManifest()) as Record<string, unknown>;
}

function stableArtifacts(manifest: Record<string, unknown>): Array<Record<string, unknown>> {
  return ((manifest.releases as Array<Record<string, unknown>>)[0]!.artifacts) as Array<
    Record<string, unknown>
  >;
}

describe('parseReleaseManifest', () => {
  test('accepts one immutable complete stable release for every desktop format', () => {
    const parsed = parseReleaseManifest(validManifest());

    expect(parsed.schemaVersion).toBe(1);
    expect(parsed.releases).toHaveLength(1);
    expect(parsed.releases[0]!.artifacts.map((entry) => `${entry.platform}:${entry.kind}`)).toEqual([
      'macos:dmg',
      'macos:updater',
      'windows:nsis',
      'windows:updater',
      'linux:appimage',
      'linux:deb',
      'linux:rpm',
      'linux:updater',
    ]);
  });

  test.each([
    [
      'unsupported schema version',
      (manifest: Record<string, unknown>) => {
        manifest.schemaVersion = 2;
      },
      /schemaVersion/,
    ],
    [
      'invalid semantic version',
      (manifest: Record<string, unknown>) => {
        (manifest.releases as Array<Record<string, unknown>>)[0]!.version = 'version-one';
      },
      /semantic version/,
    ],
    [
      'duplicate platform and kind',
      (manifest: Record<string, unknown>) => {
        const artifacts = stableArtifacts(manifest);
        artifacts.push(structuredClone(artifacts[0]!));
      },
      /duplicate/,
    ],
    [
      'non-HTTPS URL',
      (manifest: Record<string, unknown>) => {
        stableArtifacts(manifest)[0]!.url =
          'http://downloads.mdreader.dev/releases/v1.0.0/MDReader-1.0.0-universal.dmg';
      },
      /HTTPS/,
    ],
    [
      'mutable-looking URL',
      (manifest: Record<string, unknown>) => {
        stableArtifacts(manifest)[0]!.url =
          'https://downloads.mdreader.dev/releases/latest/MDReader-1.0.0-universal.dmg';
      },
      /canonical|immutable/,
    ],
    [
      'download from an untrusted HTTPS origin',
      (manifest: Record<string, unknown>) => {
        stableArtifacts(manifest)[0]!.url =
          'https://evil.example/releases/v1.0.0/MDReader-1.0.0-universal.dmg';
      },
      /downloads\.mdreader\.dev/,
    ],
    [
      'noncanonical release filename',
      (manifest: Record<string, unknown>) => {
        stableArtifacts(manifest)[0]!.url =
          'https://downloads.mdreader.dev/releases/v1.0.0/anything-1.0.0.dmg';
      },
      /canonical filename/,
    ],
    [
      'malformed checksum',
      (manifest: Record<string, unknown>) => {
        stableArtifacts(manifest)[0]!.sha256 = 'abc';
      },
      /SHA-256/,
    ],
    [
      'malformed updater signature',
      (manifest: Record<string, unknown>) => {
        stableArtifacts(manifest)[1]!.signature = 'not a detached signature';
      },
      /signature/,
    ],
    [
      'missing SBOM data',
      (manifest: Record<string, unknown>) => {
        delete stableArtifacts(manifest)[0]!.sbomUrl;
      },
      /sbomUrl/,
    ],
    [
      'filename and version disagreement',
      (manifest: Record<string, unknown>) => {
        stableArtifacts(manifest)[0]!.url =
          'https://downloads.mdreader.dev/releases/v1.0.0/MDReader-2.0.0-universal.dmg';
      },
      /filename/,
    ],
    [
      'missing Linux RPM',
      (manifest: Record<string, unknown>) => {
        const release = (manifest.releases as Array<Record<string, unknown>>)[0]!;
        release.artifacts = stableArtifacts(manifest).filter((entry) => entry.kind !== 'rpm');
      },
      /linux:rpm/,
    ],
    [
      'artifact source mismatch',
      (manifest: Record<string, unknown>) => {
        stableArtifacts(manifest)[0]!.sourceCommit =
          'ffffffffffffffffffffffffffffffffffffffff';
      },
      /source commit/,
    ],
    [
      'incomplete updater metadata',
      (manifest: Record<string, unknown>) => {
        stableArtifacts(manifest)[1]!.signature = null;
      },
      /signature/,
    ],
    [
      'zero byte artifact',
      (manifest: Record<string, unknown>) => {
        stableArtifacts(manifest)[0]!.bytes = 0;
      },
      /bytes/,
    ],
    [
      'missing minimum OS',
      (manifest: Record<string, unknown>) => {
        stableArtifacts(manifest)[0]!.minimumOs = '';
      },
      /minimumOs/,
    ],
  ])('rejects %s', (_name, mutate, message) => {
    const manifest = cloneManifest();
    mutate(manifest);
    expect(() => parseReleaseManifest(manifest)).toThrow(message);
  });

  test.each([
    'https://downloads.mdreader.dev:443/releases/v1.0.0/MDReader-1.0.0-universal.dmg',
    'https://DOWNLOADS.mdreader.dev/releases/v1.0.0/MDReader-1.0.0-universal.dmg',
    'https://downloads.mdreader.dev/releases/v1.0.0/extra/../MDReader-1.0.0-universal.dmg',
    'https://downloads.mdreader.dev/releases/v1.0.0/%2e/MDReader-1.0.0-universal.dmg',
    'https://downloads.mdreader.dev/releases/v1.0.0/%4dDReader-1.0.0-universal.dmg',
    'https://downloads.mdreader.dev/releases/v1.0.0/MDReader-1.0.0-universaℓ.dmg',
  ])('rejects normalized but noncanonical raw artifact URL %s', (url) => {
    const manifest = cloneManifest();
    stableArtifacts(manifest)[0]!.url = url;
    expect(() => parseReleaseManifest(manifest)).toThrow(/canonical/);
  });

  test('requires exactly one current stable release', () => {
    const none = cloneManifest();
    (none.releases as Array<Record<string, unknown>>)[0]!.current = false;
    expect(() => parseReleaseManifest(none)).toThrow(/exactly one current stable/);

    const two = cloneManifest();
    (two.releases as Array<Record<string, unknown>>).push(
      structuredClone((two.releases as Array<Record<string, unknown>>)[0]!),
    );
    expect(() => parseReleaseManifest(two)).toThrow(/exactly one current stable/);
  });

  test.each(['1.0.0-alpha', '1.0.0-A.2', '1.0.0+build.7'])(
    'rejects non-stable manifest version %s',
    (version) => {
      const manifest = cloneManifest();
      (manifest.releases as Array<Record<string, unknown>>)[0]!.version =
        version;
      expect(() => parseReleaseManifest(manifest)).toThrow(
        /stable semantic version/,
      );
    },
  );

  test.each([
    [
      'manifest',
      (manifest: Record<string, unknown>) => {
        manifest.unreviewed = true;
      },
    ],
    [
      'release',
      (manifest: Record<string, unknown>) => {
        (manifest.releases as Array<Record<string, unknown>>)[0]!.unreviewed =
          true;
      },
    ],
    [
      'artifact',
      (manifest: Record<string, unknown>) => {
        stableArtifacts(manifest)[0]!.unreviewed = true;
      },
    ],
  ])('rejects unknown %s fields', (_level, mutate) => {
    const manifest = cloneManifest();
    mutate(manifest);
    expect(() => parseReleaseManifest(manifest)).toThrow(/unknown fields/);
  });

  test.each([
    ['macos', 0, 'macOS 11.0'],
    ['windows', 2, 'Windows 11'],
    ['linux', 4, 'glibc 2.17'],
  ])('rejects a divergent %s minimum OS', (_platform, index, minimumOs) => {
    const manifest = cloneManifest();
    stableArtifacts(manifest)[index]!.minimumOs = minimumOs;
    expect(() => parseReleaseManifest(manifest)).toThrow(/minimumOs/);
  });
});

describe('manifest-backed website downloads', () => {
  test('absent or invalid input yields all desktop formats with zero clickable downloads', () => {
    for (const input of [undefined, null, { schemaVersion: 99 }]) {
      const cards = downloadCardsFor(downloadManifestFromRelease(input), null);
      expect(cards.map((entry) => `${entry.os}:${entry.kind}`)).toEqual([
        'macos:dmg',
        'windows:nsis',
        'linux:appimage',
        'linux:deb',
        'linux:rpm',
      ]);
      expect(cards.filter((entry) => entry.status === 'available')).toHaveLength(0);
      expect(cards.filter((entry) => entry.href !== undefined)).toHaveLength(0);
    }
  });

  test('a valid manifest is the only source of clickable URLs, exact sizes, and versions', () => {
    const stable = downloadManifestFromRelease(validManifest())!;

    expect(stable).toHaveLength(5);
    expect(
      stable.every(
        (entry) => entry.status === 'available' && entry.href?.startsWith('https://'),
      ),
    ).toBe(true);
    expect(stable.every((entry) => entry.bytes === 1_024)).toBe(true);
    expect(downloadCopy(stable)).toEqual({
      title: 'Download MDReader 1.0.0',
      lead: 'Signed installers for macOS, Windows, and Linux.',
      blurb: 'Choose a platform to download MDReader 1.0.0.',
    });
    expect(downloadCopy(stable).lead).not.toMatch(/will appear|after.*published/i);
  });

  test('OS detection changes order only and keeps every platform visible', () => {
    const artifacts = downloadManifestFromRelease(validManifest())!;
    const ordered = downloadCardsFor(artifacts, 'linux');

    expect(new Set(ordered.map((entry) => entry.os))).toEqual(
      new Set(['macos', 'windows', 'linux']),
    );
    expect(ordered.filter((entry) => entry.recommended)).toHaveLength(3);
    expect(ordered).toHaveLength(artifacts.length);
  });
});
