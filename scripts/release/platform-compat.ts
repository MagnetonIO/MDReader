function versionParts(version: string): number[] {
  return version.split('.').map((part) => Number(part));
}

function compareVersions(left: string, right: string): number {
  const a = versionParts(left);
  const b = versionParts(right);
  for (let index = 0; index < Math.max(a.length, b.length); index += 1) {
    const delta = (a[index] ?? 0) - (b[index] ?? 0);
    if (delta !== 0) return delta;
  }
  return 0;
}

export function highestRequiredGlibc(readelfReport: string): string {
  const versions = [...readelfReport.matchAll(/\bGLIBC_(\d+\.\d+(?:\.\d+)?)\b/g)].map(
    (match) => match[1]!,
  );
  if (versions.length === 0) {
    throw new Error('no GLIBC symbols were measured in the extracted application payload');
  }
  return versions.sort(compareVersions).at(-1)!;
}

export function assertGlibcCompatibility(
  readelfReport: string,
  publishedMaximum: string,
): string {
  const required = highestRequiredGlibc(readelfReport);
  if (compareVersions(required, publishedMaximum) > 0) {
    throw new Error(
      `built application requires GLIBC ${required}, newer than published minimum ${publishedMaximum}`,
    );
  }
  return required;
}
