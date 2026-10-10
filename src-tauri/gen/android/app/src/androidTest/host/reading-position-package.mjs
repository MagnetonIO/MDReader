/** Exact reviewed packages for the retained synthetic reading-position case.
 * This is not an arbitrary APK override or an installation facility. */
export const baselineSource = '7cd4ba0b7d139b5489716e2c0a11f0cd89ad39c9';
const packages = new Map([
  [baselineSource, '8758ea712a2a6dd460bb910333313eef5108d56bf22b9089b5fd57fb42841dd7'],
  ['2dcc8fcdcd3ab2cf77a1ef8ac497c5b3302fc742', 'e8a7904b8bbe6e2f2440f269e260afb5a166ba7b592c8fb9da1c23bb85933acb'],
]);

export function readingPositionPackage(source = baselineSource, resume = false) {
  const hash = packages.get(source);
  if (!hash) throw new Error('The requested shipping source is not an exact reviewed package.');
  if (source !== baselineSource && resume !== true) {
    throw new Error('The corrected candidate is restricted to the exact retained two-document resume.');
  }
  return { sourceCommit: source, apkSha256: hash };
}
