import { createHash } from 'node:crypto';
import {
  copyFile,
  mkdir,
  readFile,
  readdir,
  stat,
  writeFile,
} from 'node:fs/promises';
import { basename, join } from 'node:path';

import type {
  ReleaseArtifactKind,
  ReleaseManifestArtifact,
  ReleasePlatform,
} from '../../apps/web/lib/release-manifest';

export interface SbomPackage {
  name: string;
  version: string;
}

export interface StagePlatformInput {
  platform: ReleasePlatform;
  version: string;
  sourceCommit: string;
  publishedAt: string;
  bundleRoot: string;
  outputRoot: string;
  packages: SbomPackage[];
}

export interface ReleaseFragment {
  channel: 'stable';
  current: true;
  version: string;
  sourceCommit: string;
  artifacts: ReleaseManifestArtifact[];
}

interface ArtifactSpec {
  kind: ReleaseArtifactKind;
  label: string;
  matches: (path: string) => boolean;
  filename: (version: string) => string;
}

const SPECS: Record<ReleasePlatform, ArtifactSpec[]> = {
  macos: [
    {
      kind: 'dmg',
      label: 'macOS DMG',
      matches: (path) => path.endsWith('.dmg'),
      filename: (version) => `MDReader-${version}-universal.dmg`,
    },
    {
      kind: 'updater',
      label: 'macOS updater',
      matches: (path) => path.endsWith('.app.tar.gz'),
      filename: (version) => `MDReader-${version}-universal.app.tar.gz`,
    },
  ],
  windows: [
    {
      kind: 'nsis',
      label: 'Windows NSIS',
      matches: (path) => /setup\.exe$/i.test(path),
      filename: (version) => `MDReader-${version}-x64-setup.exe`,
    },
    {
      kind: 'updater',
      label: 'Windows updater',
      matches: (path) => path.endsWith('.nsis.zip'),
      filename: (version) => `MDReader-${version}-x64-setup.nsis.zip`,
    },
  ],
  linux: [
    {
      kind: 'appimage',
      label: 'Linux AppImage',
      matches: (path) => path.endsWith('.AppImage'),
      filename: (version) => `MDReader-${version}-x86_64.AppImage`,
    },
    {
      kind: 'deb',
      label: 'Linux DEB',
      matches: (path) => path.endsWith('.deb'),
      filename: (version) => `MDReader-${version}-x86_64.deb`,
    },
    {
      kind: 'rpm',
      label: 'Linux RPM',
      matches: (path) => path.endsWith('.rpm'),
      filename: (version) => `MDReader-${version}-x86_64.rpm`,
    },
    {
      kind: 'updater',
      label: 'Linux updater',
      matches: (path) => path.endsWith('.AppImage.tar.gz'),
      filename: (version) => `MDReader-${version}-x86_64.AppImage.tar.gz`,
    },
  ],
};

async function walk(root: string): Promise<string[]> {
  const output: string[] = [];
  for (const entry of await readdir(root, { withFileTypes: true })) {
    const path = join(root, entry.name);
    if (entry.isDirectory()) output.push(...(await walk(path)));
    else if (entry.isFile()) output.push(path);
  }
  return output;
}

function sha256(body: Uint8Array | string): string {
  return createHash('sha256').update(body).digest('hex');
}

function platformDetails(platform: ReleasePlatform): {
  architecture: 'universal' | 'x86_64';
  minimumOs: string;
} {
  if (platform === 'macos') return { architecture: 'universal', minimumOs: 'macOS 12.0' };
  if (platform === 'windows') {
    return { architecture: 'x86_64', minimumOs: 'Windows 10 1809' };
  }
  return { architecture: 'x86_64', minimumOs: 'glibc 2.31' };
}

function spdxId(value: string): string {
  return `SPDXRef-${value.replace(/[^A-Za-z0-9.-]/g, '-')}`;
}

function createSpdx(
  filename: string,
  artifactSha256: string,
  input: StagePlatformInput,
): string {
  const artifactId = spdxId(`Package-${filename}`);
  const dependencies = [...input.packages]
    .sort((left, right) => `${left.name}@${left.version}`.localeCompare(`${right.name}@${right.version}`))
    .map((pkg) => ({
      SPDXID: spdxId(`Package-${pkg.name}-${pkg.version}`),
      name: pkg.name,
      versionInfo: pkg.version,
      downloadLocation: 'NOASSERTION',
      filesAnalyzed: false,
      licenseConcluded: 'NOASSERTION',
      licenseDeclared: 'NOASSERTION',
      copyrightText: 'NOASSERTION',
    }));
  const document = {
    spdxVersion: 'SPDX-2.3',
    dataLicense: 'CC0-1.0',
    SPDXID: 'SPDXRef-DOCUMENT',
    name: `${filename} SBOM`,
    documentNamespace: `https://mdreader.dev/spdx/${input.sourceCommit}/${encodeURIComponent(filename)}`,
    creationInfo: {
      created: input.publishedAt,
      creators: ['Organization: Magneton Labs, LLC', 'Tool: mdreader-release-pipeline-1.0.0'],
    },
    packages: [
      {
        SPDXID: artifactId,
        name: filename,
        versionInfo: input.version,
        downloadLocation: 'NOASSERTION',
        filesAnalyzed: false,
        checksums: [{ algorithm: 'SHA256', checksumValue: artifactSha256 }],
        licenseConcluded: 'NOASSERTION',
        licenseDeclared: 'NOASSERTION',
        copyrightText: 'NOASSERTION',
      },
      ...dependencies,
    ],
    relationships: [
      {
        spdxElementId: 'SPDXRef-DOCUMENT',
        relationshipType: 'DESCRIBES',
        relatedSpdxElement: artifactId,
      },
      ...dependencies.map((dependency) => ({
        spdxElementId: artifactId,
        relationshipType: 'DEPENDS_ON',
        relatedSpdxElement: dependency.SPDXID,
      })),
    ],
  };
  return `${JSON.stringify(document, null, 2)}\n`;
}

async function updaterSignature(sourcePath: string): Promise<string> {
  let value;
  try {
    value = (await readFile(`${sourcePath}.sig`, 'utf8')).trim();
  } catch {
    throw new Error(`updater signature is missing for ${basename(sourcePath)}`);
  }
  if (value.length < 64 || value.length % 4 !== 0 || !/^[A-Za-z0-9+/]+={0,2}$/.test(value)) {
    throw new Error(`updater signature is malformed for ${basename(sourcePath)}`);
  }
  return value;
}

/**
 * Convert platform-native Tauri output into the immutable release layout.
 * Discovery is exact: zero or two matches fail instead of guessing.
 */
export async function stagePlatformArtifacts(
  input: StagePlatformInput,
): Promise<ReleaseFragment> {
  const files = await walk(input.bundleRoot);
  const destination = join(input.outputRoot, 'releases', `v${input.version}`);
  await mkdir(destination, { recursive: true });
  const { architecture, minimumOs } = platformDetails(input.platform);
  const artifacts: ReleaseManifestArtifact[] = [];

  for (const spec of SPECS[input.platform]) {
    const matches = files.filter(
      (path) => !path.endsWith('.sig') && spec.matches(path),
    );
    if (matches.length !== 1) {
      throw new Error(
        `expected exactly one ${spec.label}, found ${matches.length}`,
      );
    }
    const source = matches[0]!;
    const filename = spec.filename(input.version);
    const target = join(destination, filename);
    await copyFile(source, target);
    const body = await readFile(target);
    const checksum = sha256(body);
    const sbom = createSpdx(filename, checksum, input);
    const sbomFilename = `${filename}.spdx.json`;
    const sbomPath = join(destination, sbomFilename);
    await writeFile(sbomPath, sbom);
    const signature = spec.kind === 'updater' ? await updaterSignature(source) : null;
    if (signature !== null) await writeFile(`${target}.sig`, `${signature}\n`);

    artifacts.push({
      channel: 'stable',
      version: input.version,
      platform: input.platform,
      architecture,
      kind: spec.kind,
      url: `https://downloads.mdreader.dev/releases/v${input.version}/${filename}`,
      bytes: (await stat(target)).size,
      sha256: checksum,
      minimumOs,
      signature,
      sbomUrl: `https://downloads.mdreader.dev/releases/v${input.version}/${sbomFilename}`,
      sbomSha256: sha256(sbom),
      publishedAt: input.publishedAt,
      sourceCommit: input.sourceCommit,
    });
  }

  const fragment: ReleaseFragment = {
    channel: 'stable',
    current: true,
    version: input.version,
    sourceCommit: input.sourceCommit,
    artifacts,
  };
  await writeFile(
    join(input.outputRoot, `fragment-${input.platform}.json`),
    `${JSON.stringify(fragment, null, 2)}\n`,
  );
  return fragment;
}
