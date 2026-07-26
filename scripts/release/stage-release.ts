import { createHash } from 'node:crypto';
import { execFile } from 'node:child_process';
import {
  copyFile,
  mkdir,
  readFile,
  readdir,
  stat,
  writeFile,
} from 'node:fs/promises';
import { basename, join } from 'node:path';
import { promisify } from 'node:util';

import type {
  ReleaseArtifactKind,
  ReleaseManifestArtifact,
  ReleasePlatform,
} from '../../apps/web/lib/release-manifest';

const execFileAsync = promisify(execFile);
const PINNED_SYFT_CREATOR = 'Tool: syft-1.29.0';

export interface StagePlatformInput {
  platform: ReleasePlatform;
  version: string;
  sourceCommit: string;
  publishedAt: string;
  bundleRoot: string;
  outputRoot: string;
  generateSbom?: (artifactPath: string) => Promise<string>;
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
    return { architecture: 'x86_64', minimumOs: 'Windows 10' };
  }
  return { architecture: 'x86_64', minimumOs: 'glibc 2.31' };
}

function spdxId(value: string): string {
  return `SPDXRef-${value.replace(/[^A-Za-z0-9.-]/g, '-')}`;
}

function object(value: unknown, label: string): Record<string, unknown> {
  if (typeof value !== 'object' || value === null || Array.isArray(value)) {
    throw new Error(`SBOM has no complete SPDX ${label}`);
  }
  return value as Record<string, unknown>;
}

export function assertCompleteArtifactSbom(
  input: unknown,
  filename: string,
  artifactSha256: string,
): void {
  const document = object(input, 'document');
  const creation = object(document.creationInfo, 'creationInfo');
  const creators = Array.isArray(creation.creators) ? creation.creators : [];
  const packages = Array.isArray(document.packages) ? document.packages : [];
  const relationships = Array.isArray(document.relationships)
    ? document.relationships
    : [];
  if (
    document.spdxVersion !== 'SPDX-2.3' ||
    document.dataLicense !== 'CC0-1.0' ||
    document.SPDXID !== 'SPDXRef-DOCUMENT' ||
    !creators.includes(PINNED_SYFT_CREATOR) ||
    packages.length < 2
  ) {
    throw new Error('SBOM is not a complete SPDX 2.3 document from pinned Syft');
  }
  const artifact = packages
    .map((entry) => object(entry, 'package'))
    .find((entry) => entry.name === filename);
  const checksums = Array.isArray(artifact?.checksums) ? artifact.checksums : [];
  if (
    !artifact ||
    !checksums.some((entry) => {
      const checksum = object(entry, 'checksum');
      return (
        checksum.algorithm === 'SHA256' &&
        checksum.checksumValue === artifactSha256
      );
    })
  ) {
    throw new Error('SBOM does not bind the exact release artifact checksum');
  }
  if (
    !relationships.some((entry) => {
      const relationship = object(entry, 'relationship');
      return (
        relationship.spdxElementId === 'SPDXRef-DOCUMENT' &&
        relationship.relationshipType === 'DESCRIBES' &&
        relationship.relatedSpdxElement === artifact.SPDXID
      );
    })
  ) {
    throw new Error('SBOM does not describe the exact release artifact');
  }
}

async function createSpdx(
  artifactPath: string,
  filename: string,
  artifactSha256: string,
  input: StagePlatformInput,
): Promise<string> {
  const raw =
    input.generateSbom !== undefined
      ? await input.generateSbom(artifactPath)
      : (
          await execFileAsync(
            process.env.SYFT_BIN || 'syft',
            ['scan', artifactPath, '-o', 'spdx-json'],
            { maxBuffer: 64 * 1024 * 1024 },
          )
        ).stdout;
  let document: Record<string, unknown>;
  try {
    document = object(JSON.parse(raw), 'document');
  } catch {
    throw new Error(`pinned Syft did not emit valid SPDX JSON for ${filename}`);
  }
  const creation = object(document.creationInfo, 'creationInfo');
  const creators = Array.isArray(creation.creators) ? creation.creators : [];
  if (!creators.includes(PINNED_SYFT_CREATOR)) {
    throw new Error(`SBOM for ${filename} was not produced by pinned Syft 1.29.0`);
  }
  const discovered = Array.isArray(document.packages) ? document.packages : [];
  if (discovered.length === 0) {
    throw new Error(`pinned Syft found no packages in ${filename}`);
  }
  const artifactId = spdxId(`Package-${filename}`);
  document.name = `${filename} SBOM`;
  document.documentNamespace = `https://mdreader.dev/spdx/${input.sourceCommit}/${encodeURIComponent(filename)}`;
  document.packages = [
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
    ...discovered,
  ];
  document.relationships = [
    {
      spdxElementId: 'SPDXRef-DOCUMENT',
      relationshipType: 'DESCRIBES',
      relatedSpdxElement: artifactId,
    },
    ...(Array.isArray(document.relationships) ? document.relationships : []),
  ];
  assertCompleteArtifactSbom(document, filename, artifactSha256);
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
    const sbom = await createSpdx(target, filename, checksum, input);
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
