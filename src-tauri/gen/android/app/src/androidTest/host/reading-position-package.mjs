/** Exact reviewed packages for the retained synthetic reading-position case.
 * This is not an arbitrary APK override or an installation facility. */
export const baselineSource = '7cd4ba0b7d139b5489716e2c0a11f0cd89ad39c9';
const packages = new Map([
  [baselineSource, '8758ea712a2a6dd460bb910333313eef5108d56bf22b9089b5fd57fb42841dd7'],
  ['c367d56d406c01cc9b36dbfd74bc7c9bdc45060f', 'b2f07994410221d424564aad34211eba556b18f759adb45222ded193bfbb688f'],
]);

export function readingPositionPackage(source = baselineSource, resume = false) {
  const hash = packages.get(source);
  if (!hash) throw new Error('The requested shipping source is not an exact reviewed package.');
  if (source !== baselineSource && resume !== true) {
    throw new Error('The corrected candidate is restricted to the exact retained two-document resume.');
  }
  return { sourceCommit: source, apkSha256: hash };
}
